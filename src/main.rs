use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use jvl::diagnostic::{FileResult, ToolDiagnostic, Warning};
use jvl::discover::{self, CompiledSchemaMappings, Config};
use jvl::output::{self, Format, Summary, VerboseFileInfo};
use jvl::parse;
use jvl::schema::{self, SchemaCache};
use jvl::validate;

#[derive(Parser)]
#[command(name = "jvl", version, about = "JSON Schema Validator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Validate JSON files against JSON Schema
    Check(CheckArgs),

    /// Manage jvl configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// Manage the schema cache
    Cache {
        #[command(subcommand)]
        command: CacheCommands,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Print the resolved configuration
    Print(ConfigPrintArgs),

    /// Print the JSON Schema for jvl.json config files
    Schema,
}

#[derive(Subcommand)]
enum CacheCommands {
    /// Print the cache directory path
    Dir,

    /// List cached schemas
    List(CacheListArgs),

    /// Clear all cached schemas
    Clear,
}

#[derive(clap::Args)]
struct CacheListArgs {
    /// Output format
    #[arg(short = 'f', long, value_enum, default_value = "human")]
    format: Format,
}

#[derive(clap::Args)]
struct ConfigPrintArgs {
    /// Path to config file
    #[arg(short = 'c', long)]
    config: Option<PathBuf>,
}

#[derive(clap::Args)]
struct CheckArgs {
    /// File paths to validate
    files: Vec<PathBuf>,

    /// Schema to validate all files against (path or URL)
    #[arg(short = 's', long)]
    schema: Option<String>,

    /// Path to config file
    #[arg(short = 'c', long)]
    config: Option<PathBuf>,

    /// Output format
    #[arg(short = 'f', long, value_enum, default_value = "human")]
    format: Format,

    /// Number of concurrent jobs (1..=256)
    #[arg(short = 'j', long, default_value = "10", value_parser = clap::value_parser!(u16).range(1..=256))]
    jobs: u16,

    /// Error if any file has no resolvable schema
    #[arg(long)]
    strict: bool,

    /// Bypass schema cache; always fetch from network
    #[arg(long)]
    no_cache: bool,

    /// Print verbose diagnostic information to stderr
    #[arg(short = 'v', long)]
    verbose: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Check(args) => run_check(args),
        Commands::Config { command } => match command {
            ConfigCommands::Print(args) => run_config_print(args),
            ConfigCommands::Schema => run_config_schema(),
        },
        Commands::Cache { command } => match command {
            CacheCommands::Dir => run_cache_dir(),
            CacheCommands::List(args) => run_cache_list(args),
            CacheCommands::Clear => run_cache_clear(),
        },
        Commands::Completions { shell } => {
            generate(shell, &mut Cli::command(), "jvl", &mut std::io::stdout());
            ExitCode::SUCCESS
        }
    }
}

fn run_config_print(args: ConfigPrintArgs) -> ExitCode {
    let mut stderr = std::io::stderr().lock();

    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            let diag = ToolDiagnostic::error(format!("cannot determine current directory: {e}"));
            let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
            return ExitCode::from(2);
        }
    };

    let (loaded_config, _project_root) = match load_config(&args.config, &cwd) {
        Ok(result) => result,
        Err(e) => {
            let diag = ToolDiagnostic::error(format!("failed to load config: {e}"));
            let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
            return ExitCode::from(2);
        }
    };

    let config = loaded_config.unwrap_or_else(Config::default_config);
    println!("{}", serde_json::to_string_pretty(&config).unwrap());
    ExitCode::SUCCESS
}

fn run_config_schema() -> ExitCode {
    let schema = schemars::schema_for!(jvl::discover::Config);
    let mut value = serde_json::to_value(&schema).unwrap();

    // Rename definitions → $defs (2020-12 convention) and update $refs.
    rename_definitions(&mut value);

    if let Some(obj) = value.as_object_mut() {
        // Override the draft-07 meta-schema URI with 2020-12.
        obj.insert(
            "$schema".to_string(),
            serde_json::json!("https://json-schema.org/draft/2020-12/schema"),
        );
        obj.insert(
            "$id".to_string(),
            serde_json::json!("https://code.sargunv.dev/jvl/v1/jvl-config.schema.json"),
        );
    }

    println!("{}", serde_json::to_string_pretty(&value).unwrap());
    ExitCode::SUCCESS
}

fn rename_definitions(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(defs) = map.remove("definitions") {
                map.insert("$defs".to_string(), defs);
            }
            if let Some(serde_json::Value::String(ref_str)) = map.get_mut("$ref")
                && let Some(name) = ref_str.strip_prefix("#/definitions/")
            {
                *ref_str = format!("#/$defs/{name}");
            }
            for v in map.values_mut() {
                rename_definitions(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                rename_definitions(v);
            }
        }
        _ => {}
    }
}

fn run_cache_dir() -> ExitCode {
    match schema::cache_dir() {
        Some(dir) => {
            println!("{}", dir.display());
            ExitCode::SUCCESS
        }
        None => {
            let mut stderr = std::io::stderr().lock();
            let diag = ToolDiagnostic::error("cannot determine cache directory".to_string());
            let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
            ExitCode::from(2)
        }
    }
}

fn run_cache_list(args: CacheListArgs) -> ExitCode {
    let result = match schema::list_cached_schemas() {
        Ok(r) => r,
        Err(e) => {
            let mut stderr = std::io::stderr().lock();
            let diag = ToolDiagnostic::error(format!("cannot list cached schemas: {e}"));
            let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
            return ExitCode::from(2);
        }
    };

    match args.format {
        Format::Human => {
            for entry in &result.entries {
                println!(
                    "{} ({}, {})",
                    entry.url,
                    output::format_bytes(entry.size),
                    entry.fetched_at,
                );
            }

            if result.entries.is_empty() {
                eprintln!("(cache is empty)");
            } else {
                let total_size: u64 = result.entries.iter().map(|e| e.size).sum();
                eprintln!(
                    "{} cached schema{}, {} total",
                    result.entries.len(),
                    if result.entries.len() == 1 { "" } else { "s" },
                    output::format_bytes(total_size),
                );
            }

            if result.skipped > 0 {
                eprintln!(
                    "warning: {} corrupt cache entr{} skipped",
                    result.skipped,
                    if result.skipped == 1 { "y" } else { "ies" },
                );
            }
        }
        Format::Json => {
            #[derive(serde::Serialize)]
            struct JsonCacheList {
                entries: Vec<JsonCacheEntry>,
                count: usize,
                total_size: u64,
                skipped: usize,
            }

            #[derive(serde::Serialize)]
            struct JsonCacheEntry {
                url: String,
                fetched_at: String,
                size: u64,
            }

            let total_size: u64 = result.entries.iter().map(|e| e.size).sum();
            let count = result.entries.len();
            let entries: Vec<JsonCacheEntry> = result
                .entries
                .into_iter()
                .map(|e| JsonCacheEntry {
                    url: e.url,
                    fetched_at: e.fetched_at,
                    size: e.size,
                })
                .collect();

            let output = JsonCacheList {
                entries,
                count,
                total_size,
                skipped: result.skipped,
            };

            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
    }

    ExitCode::SUCCESS
}

fn run_cache_clear() -> ExitCode {
    match schema::clear_cache() {
        Ok(schema::CacheClearResult::Cleared) => {
            eprintln!("cache cleared");
            ExitCode::SUCCESS
        }
        Ok(schema::CacheClearResult::AlreadyEmpty) => {
            eprintln!("cache is already empty");
            ExitCode::SUCCESS
        }
        Err(e) => {
            let mut stderr = std::io::stderr().lock();
            let diag = ToolDiagnostic::error(format!("failed to clear cache: {e}"));
            let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
            ExitCode::from(2)
        }
    }
}

fn run_check(args: CheckArgs) -> ExitCode {
    let start = Instant::now();
    let mut stderr = std::io::stderr().lock();
    let mut early_warnings: Vec<Warning> = Vec::new();
    let verbose = args.verbose;

    // Configure rayon thread pool
    rayon::ThreadPoolBuilder::new()
        .num_threads(args.jobs as usize)
        .build_global()
        .ok(); // Ignore if already initialized

    if verbose && args.format == Format::Human {
        output::verbose_log(&mut stderr, &format!("jobs: {}", args.jobs));
    }

    // Resolve config
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            let diag = ToolDiagnostic::error(format!("cannot determine current directory: {e}"));
            let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
            return ExitCode::from(2);
        }
    };
    let (loaded_config, project_root) = match load_config(&args.config, &cwd) {
        Ok(result) => result,
        Err(e) => {
            let diag = ToolDiagnostic::error(format!("failed to load config: {e}"));
            let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
            return ExitCode::from(2);
        }
    };
    let project_root = std::fs::canonicalize(&project_root).unwrap_or(project_root);

    if verbose && args.format == Format::Human {
        match (&loaded_config, &args.config) {
            (Some(_), Some(path)) => {
                output::verbose_log(&mut stderr, &format!("config: {}", path.display()));
            }
            (Some(_), None) => {
                output::verbose_log(
                    &mut stderr,
                    &format!(
                        "config: {} (auto-discovered)",
                        project_root.join("jvl.json").display()
                    ),
                );
            }
            (None, _) => {
                output::verbose_log(&mut stderr, "config: none found, using defaults");
            }
        }
        output::verbose_log(
            &mut stderr,
            &format!("project root: {}", project_root.display()),
        );
    }

    let config = loaded_config.unwrap_or_else(Config::default_config);

    // Pre-compile schema mappings once
    let compiled_mappings = match CompiledSchemaMappings::compile(&config) {
        Ok(m) => m,
        Err(e) => {
            let diag = ToolDiagnostic::error(format!("failed to compile schema mappings: {e}"));
            let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
            return ExitCode::from(2);
        }
    };

    // Resolve schema override
    let schema_override_source = args
        .schema
        .map(|s| jvl::schema::resolve_schema_ref(&s, &cwd));

    // Discover files
    let files_to_check = if args.files.is_empty() {
        if verbose && args.format == Format::Human {
            output::verbose_log(
                &mut stderr,
                &format!("discovering files in: {}", cwd.display()),
            );
        }
        // No explicit arguments: discover from cwd
        let discover_start = Instant::now();
        match discover::discover_files(&project_root, std::slice::from_ref(&cwd), &config) {
            Ok((files, walk_warnings)) => {
                early_warnings.extend(walk_warnings);
                if verbose && args.format == Format::Human {
                    output::verbose_log(
                        &mut stderr,
                        &format!(
                            "discovered {} files ({:.0?})",
                            files.len(),
                            discover_start.elapsed()
                        ),
                    );
                }
                files
            }
            Err(e) => {
                let diag = ToolDiagnostic::error(format!("failed to discover files: {e}"));
                let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
                return ExitCode::from(2);
            }
        }
    } else {
        // Partition explicit args into directories and files
        let mut walk_roots: Vec<PathBuf> = Vec::new();
        let mut explicit_files: Vec<PathBuf> = Vec::new();

        for path in &args.files {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            if resolved.is_dir() {
                walk_roots.push(resolved);
            } else {
                explicit_files.push(path.clone());
            }
        }

        if verbose && args.format == Format::Human {
            if !explicit_files.is_empty() {
                output::verbose_log(
                    &mut stderr,
                    &format!("{} explicit files", explicit_files.len()),
                );
            }
            if !walk_roots.is_empty() {
                for root in &walk_roots {
                    output::verbose_log(
                        &mut stderr,
                        &format!("discovering files in: {}", root.display()),
                    );
                }
            }
        }

        if !walk_roots.is_empty() {
            let discover_start = Instant::now();
            match discover::discover_files(&project_root, &walk_roots, &config) {
                Ok((files, walk_warnings)) => {
                    early_warnings.extend(walk_warnings);
                    if verbose && args.format == Format::Human {
                        output::verbose_log(
                            &mut stderr,
                            &format!(
                                "discovered {} files from directories ({:.0?})",
                                files.len(),
                                discover_start.elapsed()
                            ),
                        );
                    }
                    explicit_files.extend(files);
                }
                Err(e) => {
                    let diag = ToolDiagnostic::error(format!("failed to discover files: {e}"));
                    let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
                    return ExitCode::from(2);
                }
            }
        }

        explicit_files
    };

    if files_to_check.is_empty() {
        if args.format == Format::Human {
            let diag = ToolDiagnostic::warning("no files to check".to_string());
            let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
        }
        return ExitCode::SUCCESS;
    }

    // Read all file contents upfront, stripping BOM at read time so all
    // downstream byte offsets are consistent with the stored source.
    let mut has_file_read_error = false;
    let file_contents: Vec<(String, String)> = files_to_check
        .iter()
        .filter_map(|path| {
            let path_str = path.display().to_string();
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    let content = parse::strip_bom(&content).to_owned();
                    Some((path_str, content))
                }
                Err(e) => {
                    let diag = ToolDiagnostic::error(format!("could not read {path_str}: {e}"));
                    let _ = writeln!(stderr, "{:?}", miette::Report::new(diag));
                    has_file_read_error = true;
                    None
                }
            }
        })
        .collect();

    let schema_cache = SchemaCache::new();

    // Build sources map that borrows from file_contents (no cloning)
    let sources: HashMap<&str, &str> = file_contents
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();

    // Drop the stderr lock before entering the parallel section so that
    // rayon worker threads can write verbose output without deadlocking.
    drop(stderr);

    // Process files in parallel, collecting results via rayon's lock-free collect
    let par_results: Vec<(FileResult, Vec<Warning>, Option<VerboseFileInfo>)> = file_contents
        .par_iter()
        .map(|(path, content)| {
            let file_start = if verbose { Some(Instant::now()) } else { None };

            // Determine schema for this file
            let (effective_schema, schema_via) = if let Some(ref s) = schema_override_source {
                (Some(s.clone()), "flag")
            } else {
                let relative = std::fs::canonicalize(Path::new(path))
                    .ok()
                    .and_then(|abs| {
                        abs.strip_prefix(&project_root)
                            .ok()
                            .map(|p| p.to_string_lossy().to_string())
                    })
                    .unwrap_or_else(|| path.clone());

                match compiled_mappings.resolve(&relative, &project_root) {
                    Some(s) => (Some(s), "config"),
                    None => (None, ""),
                }
            };

            let (result, file_warnings, cache_outcome, timing) = validate::validate_file(
                path,
                content,
                effective_schema.as_ref(),
                &schema_cache,
                args.no_cache,
                args.strict,
            );

            let verbose_info = if verbose {
                let file_duration = file_start.unwrap().elapsed();

                // Derive schema resolution in the caller:
                // - If effective_schema was set above → schema_via is "flag" or "config"
                // - If it was None but file wasn't skipped → inline $schema
                // - If skipped → no schema
                let (schema_display, via) = if !schema_via.is_empty() {
                    // Schema was resolved from flag or config mapping
                    (
                        effective_schema
                            .as_ref()
                            .map_or_else(String::new, |s| s.to_string()),
                        schema_via.to_string(),
                    )
                } else if !result.skipped {
                    // Schema was resolved via inline $schema field
                    let inline_ref =
                        parse::extract_schema_field_from_str(content).unwrap_or_default();
                    (inline_ref, "inline $schema".to_string())
                } else {
                    (String::new(), String::new())
                };

                // Log immediately to stderr for human format (write() is atomic for <4KB)
                if args.format == Format::Human {
                    let status = if result.skipped {
                        "skipped (no schema)"
                    } else if result.valid {
                        "valid"
                    } else if result.tool_error {
                        "error"
                    } else {
                        "invalid"
                    };

                    let schema_info = if schema_display.is_empty() && via.is_empty() {
                        "none".to_string()
                    } else if schema_display.is_empty() {
                        format!("(via {via})")
                    } else {
                        format!("{schema_display} (via {via})")
                    };

                    let cache_info =
                        cache_outcome.map_or(String::new(), |c| format!(" cache={}", c.as_str()));

                    let timing_detail = timing.as_ref().map_or(String::new(), |t| {
                        format!(
                            " (compile={:.0?}, validate={:.0?})",
                            t.compile, t.validate,
                        )
                    });

                    output::verbose_log(
                        &mut std::io::stderr(),
                        &format!(
                            "{}: {status} | schema: {schema_info} | {:.0?}{timing_detail}{cache_info}",
                            result.path, file_duration,
                        ),
                    );
                }

                Some(VerboseFileInfo {
                    schema: schema_display,
                    schema_via: via,
                    cache: cache_outcome,
                    duration: file_duration,
                    compile_duration: timing.as_ref().map(|t| t.compile),
                    validate_duration: timing.as_ref().map(|t| t.validate),
                })
            } else {
                None
            };

            (result, file_warnings, verbose_info)
        })
        .collect();

    // Re-acquire stderr lock for the rest of the function.
    let mut stderr = std::io::stderr().lock();

    let mut results = Vec::with_capacity(par_results.len());
    let mut verbose_file_infos: Vec<Option<VerboseFileInfo>> = Vec::new();
    let mut warnings = early_warnings;
    for (result, file_warnings, verbose_info) in par_results {
        results.push(result);
        warnings.extend(file_warnings);
        if verbose {
            verbose_file_infos.push(verbose_info);
        }
    }

    // Compute summary
    let checked = results.iter().filter(|r| !r.skipped).count();
    let skipped = results.iter().filter(|r| r.skipped).count();
    let invalid = results.iter().filter(|r| !r.valid && !r.skipped).count();
    let valid = checked - invalid;
    let total_errors: usize = results.iter().map(|r| r.errors.len()).sum();
    let has_tool_error = results.iter().any(|r| r.tool_error) || has_file_read_error;

    let summary = Summary {
        checked_files: checked,
        valid_files: valid,
        invalid_files: invalid,
        skipped_files: skipped,
        total_errors,
        total_warnings: warnings.len(),
        duration: start.elapsed(),
        jobs: args.jobs as usize,
        has_tool_error,
    };

    match args.format {
        Format::Human => {
            output::render_human(&results, &warnings, &summary, &sources, &mut stderr);
        }
        Format::Json => {
            let verbose_infos = if verbose {
                Some(verbose_file_infos.as_slice())
            } else {
                None
            };
            let mut stdout = std::io::stdout().lock();
            output::render_json(&results, &warnings, &summary, verbose_infos, &mut stdout);
        }
    }

    // Exit code: 2 for tool errors, 1 for validation errors, 0 for all valid
    if has_tool_error {
        ExitCode::from(2)
    } else if invalid > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Load config, returning an error if the config fails to parse.
fn load_config(
    config_path: &Option<PathBuf>,
    cwd: &Path,
) -> Result<(Option<Config>, PathBuf), discover::ConfigError> {
    if let Some(path) = config_path {
        // Explicit --config: failure is a hard error
        let cfg = Config::load(path)?;
        let abs_path = if path.is_absolute() {
            path.clone()
        } else {
            cwd.join(path)
        };
        let root = abs_path.parent().unwrap_or(cwd).to_path_buf();
        Ok((Some(cfg), root))
    } else {
        // Auto-discover: failure is non-fatal (use defaults)
        match discover::find_config_file(cwd) {
            Some(path) => match Config::load(&path) {
                Ok(cfg) => {
                    let root = path.parent().unwrap_or(cwd).to_path_buf();
                    Ok((Some(cfg), root))
                }
                Err(e) => {
                    // Auto-discovered config failed to parse — treat as tool error
                    Err(e)
                }
            },
            None => Ok((None, cwd.to_path_buf())),
        }
    }
}

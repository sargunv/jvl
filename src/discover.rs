use globset::{Glob, GlobMatcher, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::diagnostic::Warning;
use crate::schema::SchemaSource;

fn optional_string(g: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
    g.subschema_for::<String>()
}

fn uri_schema(g: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
    let mut schema: schemars::schema::SchemaObject = g.subschema_for::<String>().into();
    schema.format = Some("uri".to_string());
    schema.into()
}

fn non_empty_string_array(g: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
    let mut schema: schemars::schema::SchemaObject = g.subschema_for::<Vec<String>>().into();
    schema.array.get_or_insert_with(Default::default).min_items = Some(1);
    schema.into()
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read config file '{path}': {source}")]
    ReadError {
        path: String,
        source: std::io::Error,
    },
    #[error("Failed to parse config file '{path}': {message}")]
    JsoncParseError { path: String, message: String },
    #[error("Failed to parse config file '{path}': {source}")]
    ParseError {
        path: String,
        source: serde_json::Error,
    },
    #[error("Invalid glob pattern '{pattern}': {source}")]
    GlobError {
        pattern: String,
        source: globset::Error,
    },
}

/// Configuration file for jvl, the JSON Schema Validator.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "jvl configuration")]
pub struct Config {
    /// URL to the jvl config schema for self-validation.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    #[schemars(rename = "$schema", schema_with = "optional_string")]
    pub schema_url: Option<String>,

    /// Glob patterns for file discovery. Patterns prefixed with `!` are
    /// excludes. Order matters: later patterns override earlier ones.
    #[serde(default = "default_files")]
    pub files: Vec<String>,

    /// Schema mappings. Each entry associates a schema source (URL or local
    /// path) with a set of file glob patterns.
    #[serde(default)]
    pub schemas: Vec<SchemaMapping>,
}

fn default_files() -> Vec<String> {
    vec!["**/*.json".into(), "**/*.jsonc".into()]
}

/// A schema mapping entry. Exactly one of `url` or `path` must be present.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemaMapping {
    /// Fetch the schema from an HTTP/HTTPS URL.
    Url(SchemaMappingUrl),
    /// Load the schema from a local file path.
    Path(SchemaMappingPath),
}

/// Schema mapping using an HTTP/HTTPS URL.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaMappingUrl {
    /// HTTP/HTTPS URL to the schema (will be fetched and cached).
    #[schemars(schema_with = "uri_schema")]
    pub url: String,

    /// Glob patterns matched against each file's path relative to the project
    /// root. At least one pattern is required.
    #[schemars(schema_with = "non_empty_string_array")]
    pub files: Vec<String>,
}

/// Schema mapping using a local file path.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaMappingPath {
    /// File path to the schema, resolved relative to the project root
    /// (directory containing jvl.json).
    pub path: String,

    /// Glob patterns matched against each file's path relative to the project
    /// root. At least one pattern is required.
    #[schemars(schema_with = "non_empty_string_array")]
    pub files: Vec<String>,
}

impl SchemaMapping {
    pub fn files(&self) -> &[String] {
        match self {
            Self::Url(m) => &m.files,
            Self::Path(m) => &m.files,
        }
    }
}

impl Config {
    /// Load and parse a config file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError::ReadError {
            path: path.display().to_string(),
            source: e,
        })?;

        // Parse as JSONC (allow comments)
        let ast = jsonc_parser::parse_to_ast(
            &content,
            &Default::default(),
            &crate::parse::parse_options(),
        )
        .map_err(|e| ConfigError::JsoncParseError {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;

        let value: serde_json::Value = ast
            .value
            .ok_or_else(|| ConfigError::JsoncParseError {
                path: path.display().to_string(),
                message: "Empty config file".to_string(),
            })?
            .into();

        let config: Config =
            serde_json::from_value(value).map_err(|e| ConfigError::ParseError {
                path: path.display().to_string(),
                source: e,
            })?;

        Ok(config)
    }

    /// Default config when no config file is found.
    pub fn default_config() -> Self {
        Config {
            schema_url: None,
            files: default_files(),
            schemas: vec![],
        }
    }
}

/// Discover the config file by walking up from the start directory.
pub fn find_config_file(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?
    } else {
        start
    };

    loop {
        let candidate = dir.join("jvl.json");
        if candidate.exists() {
            return Some(candidate);
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return None,
        }
    }
}

/// Discover files using config patterns, respecting .gitignore.
///
/// Returns `(files, warnings)` where warnings include any walk errors encountered.
pub fn discover_files(
    project_root: &Path,
    config: &Config,
) -> Result<(Vec<PathBuf>, Vec<Warning>), ConfigError> {
    // Build ordered pattern list for sequential include/exclude evaluation
    let patterns = build_ordered_patterns(&config.files)?;

    let mut files = Vec::new();
    let mut warnings = Vec::new();

    // Use ignore crate for gitignore-aware walking
    let walker = WalkBuilder::new(project_root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warnings.push(Warning {
                    code: "walk".into(),
                    message: format!("Error walking directory: {e}"),
                });
                continue;
            }
        };

        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let path = entry.path();
        let relative = match path.strip_prefix(project_root) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let rel_str = relative.to_string_lossy();

        if matches_ordered_patterns(rel_str.as_ref(), &patterns) {
            files.push(path.to_path_buf());
        }
    }

    Ok((files, warnings))
}

/// Pre-compiled schema mappings for efficient per-file resolution.
pub struct CompiledSchemaMappings {
    entries: Vec<CompiledSchemaEntry>,
}

struct CompiledSchemaEntry {
    globset: GlobSet,
    mapping: SchemaMapping,
}

impl CompiledSchemaMappings {
    /// Pre-compile all schema mapping glob patterns from a config.
    pub fn compile(config: &Config) -> Result<Self, ConfigError> {
        let entries = config
            .schemas
            .iter()
            .map(|mapping| {
                let globset = build_globset(mapping.files())?;
                Ok(CompiledSchemaEntry {
                    globset,
                    mapping: mapping.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { entries })
    }

    /// Resolve a schema for a file based on pre-compiled mappings.
    pub fn resolve(&self, file_relative: &str, project_root: &Path) -> Option<SchemaSource> {
        for entry in &self.entries {
            if entry.globset.is_match(file_relative) {
                return Some(match &entry.mapping {
                    SchemaMapping::Url(m) => SchemaSource::Url(m.url.clone()),
                    SchemaMapping::Path(m) => SchemaSource::File(project_root.join(&m.path)),
                });
            }
        }
        None
    }
}

fn build_globset(patterns: &[String]) -> Result<GlobSet, ConfigError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|e| ConfigError::GlobError {
            pattern: pattern.clone(),
            source: e,
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|e| ConfigError::GlobError {
        pattern: patterns.join(", "),
        source: e,
    })
}

/// A single pattern entry for ordered evaluation: either include or exclude.
struct PatternEntry {
    exclude: bool,
    glob: GlobMatcher,
}

/// Build an ordered list of pattern entries from config file patterns.
fn build_ordered_patterns(patterns: &[String]) -> Result<Vec<PatternEntry>, ConfigError> {
    patterns
        .iter()
        .map(|pattern| {
            let (exclude, raw) = if let Some(stripped) = pattern.strip_prefix('!') {
                (true, stripped)
            } else {
                (false, pattern.as_str())
            };
            let glob = Glob::new(raw)
                .map_err(|e| ConfigError::GlobError {
                    pattern: pattern.clone(),
                    source: e,
                })?
                .compile_matcher();
            Ok(PatternEntry { exclude, glob })
        })
        .collect()
}

/// Check if a path matches the ordered pattern list.
/// Later patterns override earlier ones (include/exclude evaluated in sequence).
fn matches_ordered_patterns(path: &str, patterns: &[PatternEntry]) -> bool {
    let mut matched = false;
    for entry in patterns {
        if entry.glob.is_match(path) {
            matched = !entry.exclude;
        }
    }
    matched
}

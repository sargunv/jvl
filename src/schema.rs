use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use thiserror::Error;

use crate::diagnostic::Warning;
use crate::parse;

/// Custom retriever that routes `$ref` fetches through jvl's disk cache.
struct CachingRetriever {
    no_cache: bool,
}

impl jsonschema::Retrieve for CachingRetriever {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let url = uri.as_str();
        let (content, _warnings, _outcome) = load_url_schema(url, self.no_cache)?;
        let value: serde_json::Value = serde_json::from_str(&content)?;
        Ok(value)
    }
}

/// Describes how a URL schema was resolved from cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOutcome {
    /// Schema was served from a fresh disk-cache entry.
    Hit,
    /// Schema was not in cache and was fetched from the network.
    Miss,
    /// Disk-cache entry was stale; a re-fetch was attempted.
    Stale,
    /// Cache was explicitly bypassed via --no-cache.
    Bypassed,
}

impl CacheOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Stale => "stale",
            Self::Bypassed => "bypassed",
        }
    }
}

/// The result of compiling (or retrieving) a schema: the compiled validator,
/// any warnings emitted during compilation, and an optional cache outcome.
pub type CompileResult = (
    Arc<jsonschema::Validator>,
    Vec<Warning>,
    Option<CacheOutcome>,
);

#[derive(Debug, Clone, Error)]
pub enum SchemaError {
    #[error("Failed to read schema file '{path}': {reason}")]
    FileRead { path: String, reason: String },
    #[error("Failed to parse schema from '{path}': {reason}")]
    ParseError { path: String, reason: String },
    #[error("Failed to fetch schema from '{url}': {reason}")]
    FetchError { url: String, reason: String },
    #[error("Failed to compile schema: {0}")]
    CompileError(String),
}

impl SchemaError {
    /// The underlying reason without the schema path/URL prefix.
    /// Use this when the schema location is already shown in a source span.
    pub fn reason(&self) -> &str {
        match self {
            SchemaError::FileRead { reason, .. }
            | SchemaError::ParseError { reason, .. }
            | SchemaError::FetchError { reason, .. } => reason,
            SchemaError::CompileError(msg) => msg,
        }
    }
}

/// Normalize a file path for use as a cache key.
///
/// Uses `std::fs::canonicalize` when the file exists (resolves symlinks,
/// case, and `.`/`..` segments). Falls back to lexical normalization
/// (strip `.` and `..` segments without I/O) for non-existent paths.
pub(crate) fn normalize_file_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize_lexical(path))
}

/// Strip `.` and `..` segments from an absolute path without touching the filesystem.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                components.pop();
            }
            c => components.push(c),
        }
    }
    components.iter().collect()
}

/// Where the schema came from.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum SchemaSource {
    /// A local file path (absolute).
    File(PathBuf),
    /// An HTTP/HTTPS URL.
    Url(String),
}

impl SchemaSource {
    /// Create a `File` source with a normalized path.
    pub fn file(path: PathBuf) -> Self {
        SchemaSource::File(normalize_file_path(&path))
    }
}

impl std::fmt::Display for SchemaSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaSource::File(p) => write!(f, "{}", p.display()),
            SchemaSource::Url(u) => write!(f, "{u}"),
        }
    }
}

/// Resolve a `$schema` string to a SchemaSource.
///
/// - URLs (http:// or https://) become `SchemaSource::Url`.
/// - Absolute paths become `SchemaSource::File`.
/// - Relative paths are resolved relative to `base_dir`.
pub fn resolve_schema_ref(schema_ref: &str, base_dir: &Path) -> SchemaSource {
    if schema_ref.starts_with("http://") || schema_ref.starts_with("https://") {
        SchemaSource::Url(schema_ref.to_string())
    } else {
        let path = Path::new(schema_ref);
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        };
        SchemaSource::file(abs)
    }
}

/// Try to resolve a schema source from the `$schema` field in a parsed JSON value.
///
/// Returns `None` if the value has no `$schema` string field. Relative paths
/// in the `$schema` value are resolved against the parent directory of
/// `file_path`.
pub fn resolve_schema_from_value(
    parsed_value: &serde_json::Value,
    file_path: &Path,
) -> Option<SchemaSource> {
    parse::extract_schema_field(parsed_value).map(|schema_ref| {
        let base_dir = file_path.parent().unwrap_or(Path::new("."));
        resolve_schema_ref(schema_ref, base_dir)
    })
}

/// Cache directory for fetched schemas.
pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("jvl").join("schemas"))
}

/// SHA-256 hex digest of a URL, used as cache key.
fn url_hash(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Metadata for a cached schema (internal on-disk format).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CacheMeta {
    url: String,
    fetched_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
}

/// Information about a single cached schema entry.
#[derive(Debug)]
pub struct CachedSchemaInfo {
    /// The original URL of the cached schema.
    pub url: String,
    /// ISO 8601 timestamp of when the schema was fetched.
    pub fetched_at: String,
    /// Size of the cached schema content in bytes.
    pub size: u64,
}

/// Result of listing cached schemas.
pub struct CacheListResult {
    /// Successfully read cache entries, sorted by URL.
    pub entries: Vec<CachedSchemaInfo>,
    /// Number of `.meta` files that could not be read or parsed.
    pub skipped: usize,
}

/// Result of clearing the cache.
pub enum CacheClearResult {
    /// Cache was cleared successfully.
    Cleared,
    /// Cache directory did not exist (already empty).
    AlreadyEmpty,
}

/// TTL for cached schemas: 24 hours.
const CACHE_TTL_SECS: i64 = 24 * 60 * 60;

/// HTTP request timeout.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Load schema content from a source, using disk cache for URLs.
///
/// Returns `(schema_json_string, warnings, cache_outcome)`.
/// `cache_outcome` is `None` for file-based schemas (no caching involved).
fn load_schema_content(
    source: &SchemaSource,
    no_cache: bool,
) -> Result<(String, Vec<Warning>, Option<CacheOutcome>), SchemaError> {
    match source {
        SchemaSource::File(path) => {
            let content = fs::read_to_string(path).map_err(|e| SchemaError::FileRead {
                path: path.display().to_string(),
                reason: e.to_string(),
            })?;
            Ok((content, vec![], None))
        }
        SchemaSource::Url(url) => load_url_schema(url, no_cache),
    }
}

fn load_url_schema(
    url: &str,
    no_cache: bool,
) -> Result<(String, Vec<Warning>, Option<CacheOutcome>), SchemaError> {
    let hash = url_hash(url);

    if no_cache {
        let content = fetch_url(url)?;
        return Ok((content, vec![], Some(CacheOutcome::Bypassed)));
    }

    let cache_base = cache_dir();

    // Try to read from cache
    if let Some(ref base) = cache_base {
        let schema_path = base.join(format!("{hash}.json"));
        let meta_path = base.join(format!("{hash}.meta"));

        if schema_path.exists() {
            let cached_content = fs::read_to_string(&schema_path).ok();

            if let Some(content) = cached_content {
                if is_within_ttl(&meta_path) {
                    return Ok((content, vec![], Some(CacheOutcome::Hit)));
                }

                // Stale: attempt re-fetch. Use fresh content if successful,
                // fall back to stale content on failure.
                match fetch_url(url) {
                    Ok(fresh) => {
                        let _ = write_cache(base, &hash, url, &fresh);
                        return Ok((fresh, vec![], Some(CacheOutcome::Stale)));
                    }
                    Err(_) => {
                        let warning = Warning {
                            code: "cache(stale)".into(),
                            message: format!(
                                "Using stale cached schema for {url} (re-fetch failed)"
                            ),
                        };
                        return Ok((content, vec![warning], Some(CacheOutcome::Stale)));
                    }
                }
            }
        }
    }

    // No cache hit — fetch synchronously
    let content = fetch_url(url)?;

    // Write to cache
    if let Some(ref base) = cache_base {
        let _ = write_cache(base, &hash, url, &content);
    }

    Ok((content, vec![], Some(CacheOutcome::Miss)))
}

fn is_within_ttl(meta_path: &Path) -> bool {
    let meta_str = fs::read_to_string(meta_path).ok();
    let meta = meta_str
        .as_deref()
        .and_then(|s| serde_json::from_str::<CacheMeta>(s).ok());
    let fetched_at = meta
        .as_ref()
        .and_then(|m| m.fetched_at.parse::<jiff::Timestamp>().ok());
    match fetched_at {
        Some(ts) => ts.duration_until(jiff::Timestamp::now()).as_secs() < CACHE_TTL_SECS,
        None => false,
    }
}

static HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

fn get_http_client() -> &'static reqwest::blocking::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .expect("failed to build HTTP client")
    })
}

fn fetch_url(url: &str) -> Result<String, SchemaError> {
    let client = get_http_client();

    let resp = client
        .get(url)
        .send()
        .map_err(|e| SchemaError::FetchError {
            url: url.to_string(),
            reason: e.to_string(),
        })?;
    if !resp.status().is_success() {
        return Err(SchemaError::FetchError {
            url: url.to_string(),
            reason: format!("HTTP {}", resp.status()),
        });
    }
    resp.text().map_err(|e| SchemaError::FetchError {
        url: url.to_string(),
        reason: e.to_string(),
    })
}

fn write_cache(base: &Path, hash: &str, url: &str, content: &str) -> Result<(), std::io::Error> {
    fs::create_dir_all(base)?;
    let schema_path = base.join(format!("{hash}.json"));
    let meta_path = base.join(format!("{hash}.meta"));

    // Refuse to follow symlinks (defense-in-depth, consistent with clear_cache).
    for path in [&schema_path, &meta_path] {
        if let Ok(m) = fs::symlink_metadata(path)
            && m.file_type().is_symlink()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "cache file is a symlink; refusing to write: {}",
                    path.display()
                ),
            ));
        }
    }

    fs::write(&schema_path, content)?;
    let meta = CacheMeta {
        url: url.to_string(),
        fetched_at: jiff::Timestamp::now().to_string(),
        etag: None,
    };
    let meta_json = serde_json::to_string_pretty(&meta).unwrap();
    fs::write(&meta_path, meta_json)?;
    Ok(())
}

/// List all cached schemas from the disk cache.
///
/// Returns entries sorted by URL. Entries with corrupt or unreadable `.meta`
/// files are counted in `skipped` rather than silently ignored.
pub fn list_cached_schemas() -> Result<CacheListResult, std::io::Error> {
    let dir = cache_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cannot determine cache directory",
        )
    })?;

    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CacheListResult {
                entries: vec![],
                skipped: 0,
            });
        }
        Err(e) => return Err(e),
    };

    let mut infos: Vec<CachedSchemaInfo> = Vec::new();
    let mut skipped: usize = 0;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let meta: CacheMeta = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };

        let json_path = path.with_extension("json");
        let size = fs::metadata(&json_path).map(|m| m.len()).unwrap_or(0);

        infos.push(CachedSchemaInfo {
            url: meta.url,
            fetched_at: meta.fetched_at,
            size,
        });
    }

    infos.sort_unstable_by(|a, b| a.url.cmp(&b.url));

    Ok(CacheListResult {
        entries: infos,
        skipped,
    })
}

/// Clear all cached schemas from the disk cache.
///
/// Refuses to operate if the cache directory is a symlink (defense-in-depth).
pub fn clear_cache() -> Result<CacheClearResult, std::io::Error> {
    let dir = cache_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cannot determine cache directory",
        )
    })?;

    // Check for symlink before deletion (defense-in-depth).
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "cache directory is a symlink; refusing to clear",
            ));
        }
        Ok(_) => {} // real directory, proceed
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CacheClearResult::AlreadyEmpty);
        }
        Err(e) => return Err(e),
    }

    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(CacheClearResult::Cleared),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CacheClearResult::AlreadyEmpty),
        Err(e) => Err(e),
    }
}

/// Thread-safe cache of compiled schema validators.
///
/// Uses per-slot `OnceLock` to ensure each schema is fetched and compiled
/// exactly once, even under concurrent access from multiple rayon threads.
#[derive(Default)]
pub struct SchemaCache {
    slots: Mutex<HashMap<SchemaSource, Arc<SchemaSlot>>>,
}

struct SchemaSlot {
    compiled: OnceLock<SlotResult>,
    /// Ensures only the initializing thread reports warnings.
    warnings_taken: AtomicBool,
}

struct SlotResult {
    validator: Result<Arc<jsonschema::Validator>, SchemaError>,
    schema_value: Option<Arc<serde_json::Value>>,
    warnings: Vec<Warning>,
    cache_outcome: Option<CacheOutcome>,
}

impl SchemaCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return all `SchemaSource::File` paths currently in the cache.
    pub fn cached_file_paths(&self) -> Vec<PathBuf> {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter_map(|source| match source {
                SchemaSource::File(p) => Some(p.clone()),
                _ => None,
            })
            .collect()
    }

    /// Evict a cached schema slot, forcing recompilation on the next
    /// [`get_or_compile`](Self::get_or_compile) call for this source.
    ///
    /// Returns `true` if the source was present in the cache.
    pub fn evict(&self, source: &SchemaSource) -> bool {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(source)
            .is_some()
    }

    /// Retrieve the raw schema JSON for a previously compiled source.
    ///
    /// Returns `None` if the source has not been compiled yet or if compilation
    /// failed before the schema could be parsed.
    pub fn get_schema_value(&self, source: &SchemaSource) -> Option<Arc<serde_json::Value>> {
        let slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let slot = slots.get(source)?;
        let result = slot.compiled.get()?;
        result.schema_value.clone()
    }

    /// Get or compile the schema and return the raw schema JSON value.
    ///
    /// Combines [`get_or_compile`](Self::get_or_compile) and
    /// [`get_schema_value`](Self::get_schema_value) in a single operation.
    pub fn get_or_compile_with_value(
        &self,
        source: &SchemaSource,
        no_cache: bool,
    ) -> Result<Option<Arc<serde_json::Value>>, SchemaError> {
        let _ = self.get_or_compile(source, no_cache)?;
        Ok(self.get_schema_value(source))
    }

    /// Get or load+compile a schema validator.
    ///
    /// Returns `(validator, warnings, cache_outcome)`. The validator is wrapped
    /// in `Arc` for cheap cloning across threads. Warnings are only returned to
    /// the first caller (the one that triggered compilation). `cache_outcome` is
    /// `None` for file-based schemas or when the result was already compiled
    /// in-memory by another thread.
    pub fn get_or_compile(
        &self,
        source: &SchemaSource,
        no_cache: bool,
    ) -> Result<CompileResult, SchemaError> {
        let slot = {
            let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            slots
                .entry(source.clone())
                .or_insert_with(|| {
                    Arc::new(SchemaSlot {
                        compiled: OnceLock::new(),
                        warnings_taken: AtomicBool::new(false),
                    })
                })
                .clone()
        };

        // OnceLock::get_or_init guarantees exactly one thread runs the closure.
        // Other threads calling concurrently will block until init completes.
        let result = slot.compiled.get_or_init(|| {
            let (content, warnings, cache_outcome) = match load_schema_content(source, no_cache) {
                Ok(r) => r,
                Err(e) => {
                    return SlotResult {
                        validator: Err(e),
                        schema_value: None,
                        warnings: vec![],
                        cache_outcome: None,
                    };
                }
            };

            let schema_value: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(e) => {
                    return SlotResult {
                        validator: Err(SchemaError::ParseError {
                            path: source.to_string(),
                            reason: e.to_string(),
                        }),
                        schema_value: None,
                        warnings,
                        cache_outcome,
                    };
                }
            };

            let schema_value = Arc::new(schema_value);

            let validator = match jsonschema::options()
                .with_retriever(CachingRetriever { no_cache })
                .build(&schema_value)
            {
                Ok(v) => v,
                Err(e) => {
                    return SlotResult {
                        validator: Err(SchemaError::CompileError(e.to_string())),
                        schema_value: Some(Arc::clone(&schema_value)),
                        warnings,
                        cache_outcome,
                    };
                }
            };

            SlotResult {
                validator: Ok(Arc::new(validator)),
                schema_value: Some(schema_value),
                warnings,
                cache_outcome,
            }
        });

        // Only the first caller to reach here takes the warnings.
        let is_first = !slot.warnings_taken.swap(true, Ordering::Relaxed);
        let warnings = if is_first {
            result.warnings.clone()
        } else {
            vec![]
        };

        // Only the initializing thread gets the real cache outcome;
        // subsequent threads see None (already compiled in memory).
        let cache_outcome = if is_first { result.cache_outcome } else { None };

        match &result.validator {
            Ok(v) => Ok((Arc::clone(v), warnings, cache_outcome)),
            Err(e) => Err(e.clone()),
        }
    }
}

/// Schema annotations (title/description) for a JSON path.
pub struct SchemaAnnotation {
    pub title: Option<String>,
    pub description: Option<String>,
}

impl SchemaAnnotation {
    /// Format as Markdown for LSP hover display.
    ///
    /// Returns `None` if both `title` and `description` are `None`.
    pub fn to_markdown(&self) -> Option<String> {
        const MAX_LEN: usize = 10_000;
        let truncate = |s: &str| -> String {
            if s.len() > MAX_LEN {
                let end = s.floor_char_boundary(MAX_LEN);
                format!("{}...", &s[..end])
            } else {
                s.to_string()
            }
        };

        match (self.title.as_deref(), self.description.as_deref()) {
            (Some(t), Some(d)) => Some(format!("**{}**\n\n{}", truncate(t), truncate(d))),
            (Some(t), None) => Some(format!("**{}**", truncate(t))),
            (None, Some(d)) => Some(truncate(d)),
            (None, None) => None,
        }
    }
}

/// Walk a raw schema `serde_json::Value` following a JSON pointer path and
/// return structured schema annotations (title/description).
///
/// Handles `properties`, `items`/`prefixItems`, and fragment-only `$ref`.
/// Returns `None` if no `title` or `description` annotation is found.
pub fn lookup_schema_annotation(
    schema: &serde_json::Value,
    pointer: &[String],
) -> Option<SchemaAnnotation> {
    let mut visited_refs: HashSet<String> = HashSet::new();
    let subschema = resolve_subschema(schema, schema, pointer, &mut visited_refs, 0)?;

    let title = subschema
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let description = subschema
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if title.is_none() && description.is_none() {
        return None;
    }

    Some(SchemaAnnotation { title, description })
}

/// Walk a raw schema `serde_json::Value` following a JSON pointer path and
/// return formatted hover content (title/description as Markdown).
///
/// Handles `properties`, `items`/`prefixItems`, `allOf`, and fragment-only `$ref`.
/// Returns `None` if no `title` or `description` annotation is found.
pub fn lookup_hover_content(schema: &serde_json::Value, pointer: &[String]) -> Option<String> {
    lookup_schema_annotation(schema, pointer)?.to_markdown()
}

/// Navigate to the subschema at a JSON pointer path, following `$ref` and `allOf`.
///
/// Public entry point that initializes cycle-detection state internally.
/// Returns `None` if the path cannot be resolved.
pub fn resolve_subschema_at_pointer<'a>(
    root: &'a serde_json::Value,
    pointer: &[String],
) -> Option<&'a serde_json::Value> {
    let mut visited = HashSet::new();
    resolve_subschema(root, root, pointer, &mut visited, 0)
}

/// Maximum recursion depth for schema traversal (defense against pathological schemas).
const MAX_SCHEMA_DEPTH: usize = 32;

/// Information about a completable property from a schema.
#[derive(Debug, Clone)]
pub struct PropertyInfo {
    pub name: String,
    pub required: bool,
    pub description: Option<String>,
    pub schema_type: Option<String>,
}

/// Possible value suggestions for a property.
#[derive(Debug, Clone)]
pub enum ValueSuggestion {
    /// A value from a schema `enum` array.
    Enum(serde_json::Value),
    /// A value from a schema `const`.
    Const(serde_json::Value),
    /// Suggests `true` and `false`.
    Boolean,
    /// Suggests `null`.
    Null,
}

/// Collect all completable properties from a schema at the given pointer path.
///
/// Follows `$ref` and merges `allOf` branches. Returns an empty vec if the
/// resolved subschema is not an object schema or the pointer cannot be resolved.
pub fn collect_properties(root: &serde_json::Value, pointer: &[String]) -> Vec<PropertyInfo> {
    let mut visited = HashSet::new();
    let Some(subschema) = resolve_subschema(root, root, pointer, &mut visited, 0) else {
        return vec![];
    };

    let mut props: Vec<PropertyInfo> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    collect_properties_from(root, subschema, &mut props, &mut seen_names, 0);
    props
}

/// Recursively collect properties from a schema, merging allOf branches.
fn collect_properties_from(
    root: &serde_json::Value,
    schema: &serde_json::Value,
    props: &mut Vec<PropertyInfo>,
    seen: &mut HashSet<String>,
    depth: usize,
) {
    if depth > MAX_SCHEMA_DEPTH {
        return;
    }

    // Follow $ref.
    let mut visited = HashSet::new();
    let Some(schema) = follow_ref(root, schema, &mut visited) else {
        return;
    };

    // Collect required set for this schema.
    let required: HashSet<&str> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    // Collect properties.
    if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
        for (name, prop_schema) in properties {
            if !seen.insert(name.clone()) {
                continue; // Already seen from an earlier allOf branch.
            }

            // Follow $ref on the property schema for metadata.
            let mut prop_visited = HashSet::new();
            let resolved_prop =
                follow_ref(root, prop_schema, &mut prop_visited).unwrap_or(prop_schema);

            let description = format_annotation(resolved_prop);
            let schema_type = resolved_prop
                .get("type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            props.push(PropertyInfo {
                name: name.clone(),
                required: required.contains(name.as_str()),
                description,
                schema_type,
            });
        }
    }

    // Recurse into allOf branches.
    if let Some(all_of) = schema.get("allOf").and_then(|v| v.as_array()) {
        for branch in all_of {
            collect_properties_from(root, branch, props, seen, depth + 1);
        }
    }
}

/// Format title and description from a schema node into a single string.
fn format_annotation(schema: &serde_json::Value) -> Option<String> {
    let title = schema.get("title").and_then(|v| v.as_str());
    let description = schema.get("description").and_then(|v| v.as_str());
    match (title, description) {
        (Some(t), Some(d)) => Some(format!("{t}: {d}")),
        (Some(t), None) => Some(t.to_string()),
        (None, Some(d)) => Some(d.to_string()),
        (None, None) => None,
    }
}

/// Collect possible value suggestions for a property at the given pointer path.
///
/// Navigates to the subschema at `pointer` + `property_name`, then inspects
/// `enum`, `const`, and `type` to generate value suggestions.
pub fn collect_values(
    root: &serde_json::Value,
    pointer: &[String],
    property_name: &str,
) -> Vec<ValueSuggestion> {
    let mut full_pointer: Vec<String> = pointer.to_vec();
    full_pointer.push(property_name.to_string());

    let Some(subschema) = resolve_subschema_at_pointer(root, &full_pointer) else {
        return vec![];
    };

    let mut suggestions = Vec::new();

    // Check for const.
    if let Some(const_val) = subschema.get("const") {
        suggestions.push(ValueSuggestion::Const(const_val.clone()));
        return suggestions;
    }

    // Check for enum.
    if let Some(enum_vals) = subschema.get("enum").and_then(|v| v.as_array()) {
        for val in enum_vals {
            suggestions.push(ValueSuggestion::Enum(val.clone()));
        }
        return suggestions;
    }

    // Check for type-based suggestions.
    if let Some(type_str) = subschema.get("type").and_then(|v| v.as_str()) {
        match type_str {
            "boolean" => suggestions.push(ValueSuggestion::Boolean),
            "null" => suggestions.push(ValueSuggestion::Null),
            _ => {}
        }
    }

    suggestions
}

/// Resolve the subschema at a given JSON pointer path within a schema.
fn resolve_subschema<'a>(
    root: &'a serde_json::Value,
    schema: &'a serde_json::Value,
    pointer: &[String],
    visited: &mut HashSet<String>,
    depth: usize,
) -> Option<&'a serde_json::Value> {
    if depth > MAX_SCHEMA_DEPTH {
        return None;
    }

    // Follow $ref before descending.
    let schema = follow_ref(root, schema, visited)?;

    if pointer.is_empty() {
        return Some(schema);
    }

    let segment = &pointer[0];
    let rest = &pointer[1..];

    // Try properties.<segment>
    if let Some(prop_schema) = schema
        .get("properties")
        .and_then(|p| p.get(segment.as_str()))
    {
        return resolve_subschema(root, prop_schema, rest, visited, depth + 1);
    }

    // Try allOf branches.
    if let Some(all_of) = schema.get("allOf").and_then(|v| v.as_array()) {
        for branch in all_of {
            if let Some(result) = resolve_subschema(root, branch, pointer, visited, depth + 1) {
                return Some(result);
            }
        }
    }

    // Try items (for array elements with numeric index).
    if let Ok(idx) = segment.parse::<usize>() {
        if let Some(items) = schema.get("items")
            && (items.is_object() || items.is_boolean())
        {
            return resolve_subschema(root, items, rest, visited, depth + 1);
        }
        // Try prefixItems[i]
        if let Some(prefix_items) = schema.get("prefixItems").and_then(|p| p.as_array())
            && let Some(item_schema) = prefix_items.get(idx)
        {
            return resolve_subschema(root, item_schema, rest, visited, depth + 1);
        }
    }

    None
}

/// Follow `$ref` within the same document. Only fragment references (`#/...`)
/// are followed. Returns the resolved schema, or the input schema if no `$ref`.
fn follow_ref<'a>(
    root: &'a serde_json::Value,
    schema: &'a serde_json::Value,
    visited: &mut HashSet<String>,
) -> Option<&'a serde_json::Value> {
    let ref_str = match schema.get("$ref").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return Some(schema),
    };

    // Only follow fragment references within the same document.
    if !ref_str.starts_with('#') {
        return Some(schema);
    }

    // Cycle detection.
    if !visited.insert(ref_str.to_string()) {
        return None;
    }

    // Parse the JSON Pointer from the fragment (strip leading `#/`).
    let pointer_str = ref_str.strip_prefix("#/").unwrap_or("");
    if pointer_str.is_empty() {
        return Some(root);
    }

    let mut target = root;
    for segment in pointer_str.split('/') {
        // Unescape JSON Pointer encoding (~1 = /, ~0 = ~).
        let unescaped = segment.replace("~1", "/").replace("~0", "~");
        target = target.get(&unescaped)?;
    }

    // Recursively follow if the target itself has a $ref.
    follow_ref(root, target, visited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn evict_forces_recompilation_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let schema_path = dir.path().join("schema.json");

        // Write a schema that requires {"name": string}.
        std::fs::write(
            &schema_path,
            r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"#,
        )
        .unwrap();

        let cache = SchemaCache::new();
        let source = SchemaSource::File(schema_path.clone());

        // First compile should succeed and cache the validator.
        let (validator_v1, _, _) = cache.get_or_compile(&source, true).unwrap();

        // Valid doc passes.
        let doc: serde_json::Value = serde_json::from_str(r#"{"name":"alice"}"#).unwrap();
        assert!(validator_v1.is_valid(&doc));

        // Now overwrite the schema to require {"count": number} instead.
        let mut f = std::fs::File::create(&schema_path).unwrap();
        f.write_all(
            br#"{"type":"object","properties":{"count":{"type":"number"}},"required":["count"]}"#,
        )
        .unwrap();
        drop(f);

        // Without eviction, the cache still returns the old validator.
        let (validator_stale, _, _) = cache.get_or_compile(&source, true).unwrap();
        assert!(
            validator_stale.is_valid(&doc),
            "stale validator should still accept old doc"
        );

        // Evict and recompile — now the new schema should be used.
        assert!(cache.evict(&source));
        let (validator_v2, _, _) = cache.get_or_compile(&source, true).unwrap();

        // Old doc is now invalid (missing "count").
        assert!(!validator_v2.is_valid(&doc));

        // New doc passes.
        let new_doc: serde_json::Value = serde_json::from_str(r#"{"count":42}"#).unwrap();
        assert!(validator_v2.is_valid(&new_doc));
    }

    #[test]
    fn evict_nonexistent_returns_false() {
        let cache = SchemaCache::new();
        let source = SchemaSource::File(PathBuf::from("/nonexistent/schema.json"));
        assert!(!cache.evict(&source));
    }

    #[test]
    fn hover_simple_property() {
        let schema = serde_json::json!({
            "properties": {
                "name": {
                    "title": "Name",
                    "description": "The user's name"
                }
            }
        });
        let result = lookup_hover_content(&schema, &["name".into()]);
        assert_eq!(result, Some("**Name**\n\nThe user's name".into()));
    }

    #[test]
    fn hover_nested_property() {
        let schema = serde_json::json!({
            "properties": {
                "server": {
                    "properties": {
                        "host": {
                            "title": "Host",
                            "description": "Server hostname"
                        }
                    }
                }
            }
        });
        let result = lookup_hover_content(&schema, &["server".into(), "host".into()]);
        assert_eq!(result, Some("**Host**\n\nServer hostname".into()));
    }

    #[test]
    fn hover_ref_resolution() {
        let schema = serde_json::json!({
            "$defs": {
                "Address": {
                    "title": "Address",
                    "description": "A postal address"
                }
            },
            "properties": {
                "home": { "$ref": "#/$defs/Address" }
            }
        });
        let result = lookup_hover_content(&schema, &["home".into()]);
        assert_eq!(result, Some("**Address**\n\nA postal address".into()));
    }

    #[test]
    fn hover_ref_cycle_returns_none() {
        let schema = serde_json::json!({
            "$defs": {
                "A": { "$ref": "#/$defs/B" },
                "B": { "$ref": "#/$defs/A" }
            },
            "properties": {
                "x": { "$ref": "#/$defs/A" }
            }
        });
        let result = lookup_hover_content(&schema, &["x".into()]);
        assert!(result.is_none());
    }

    #[test]
    fn hover_external_ref_ignored() {
        let schema = serde_json::json!({
            "properties": {
                "x": {
                    "$ref": "https://evil.example.com/schema",
                    "title": "Fallback"
                }
            }
        });
        // External $ref is ignored; sibling title is shown.
        let result = lookup_hover_content(&schema, &["x".into()]);
        assert_eq!(result, Some("**Fallback**".into()));
    }

    #[test]
    fn hover_no_annotation_returns_none() {
        let schema = serde_json::json!({
            "properties": {
                "count": { "type": "number" }
            }
        });
        let result = lookup_hover_content(&schema, &["count".into()]);
        assert!(result.is_none());
    }

    #[test]
    fn hover_title_only() {
        let schema = serde_json::json!({
            "properties": {
                "x": { "title": "Just a title" }
            }
        });
        let result = lookup_hover_content(&schema, &["x".into()]);
        assert_eq!(result, Some("**Just a title**".into()));
    }

    #[test]
    fn hover_description_only() {
        let schema = serde_json::json!({
            "properties": {
                "x": { "description": "Just a description" }
            }
        });
        let result = lookup_hover_content(&schema, &["x".into()]);
        assert_eq!(result, Some("Just a description".into()));
    }

    #[test]
    fn hover_items_array() {
        let schema = serde_json::json!({
            "properties": {
                "tags": {
                    "items": {
                        "title": "Tag",
                        "description": "A tag string"
                    }
                }
            }
        });
        let result = lookup_hover_content(&schema, &["tags".into(), "0".into()]);
        assert_eq!(result, Some("**Tag**\n\nA tag string".into()));
    }

    #[test]
    fn hover_prefix_items() {
        let schema = serde_json::json!({
            "properties": {
                "coords": {
                    "prefixItems": [
                        { "title": "X", "description": "X coordinate" },
                        { "title": "Y", "description": "Y coordinate" }
                    ]
                }
            }
        });
        let result = lookup_hover_content(&schema, &["coords".into(), "1".into()]);
        assert_eq!(result, Some("**Y**\n\nY coordinate".into()));
    }

    #[test]
    fn hover_allof_property() {
        let schema = serde_json::json!({
            "allOf": [
                {
                    "properties": {
                        "name": {
                            "title": "Name",
                            "description": "User name"
                        }
                    }
                },
                {
                    "properties": {
                        "age": {
                            "title": "Age",
                            "description": "User age"
                        }
                    }
                }
            ]
        });
        // Property defined in first allOf branch.
        let result = lookup_hover_content(&schema, &["name".into()]);
        assert_eq!(result, Some("**Name**\n\nUser name".into()));
        // Property defined in second allOf branch.
        let result = lookup_hover_content(&schema, &["age".into()]);
        assert_eq!(result, Some("**Age**\n\nUser age".into()));
    }

    #[test]
    fn hover_allof_nested() {
        let schema = serde_json::json!({
            "properties": {
                "server": {
                    "allOf": [
                        {
                            "properties": {
                                "host": {
                                    "title": "Host",
                                    "description": "Server hostname"
                                }
                            }
                        }
                    ]
                }
            }
        });
        let result = lookup_hover_content(&schema, &["server".into(), "host".into()]);
        assert_eq!(result, Some("**Host**\n\nServer hostname".into()));
    }

    #[test]
    fn resolve_subschema_at_pointer_basic() {
        let schema = serde_json::json!({
            "properties": {
                "name": { "type": "string", "title": "Name" }
            }
        });
        let result = resolve_subschema_at_pointer(&schema, &["name".into()]);
        assert!(result.is_some());
        assert_eq!(result.unwrap().get("title").unwrap(), "Name");
    }

    #[test]
    fn resolve_subschema_at_pointer_empty() {
        let schema = serde_json::json!({ "type": "object" });
        let result = resolve_subschema_at_pointer(&schema, &[]);
        assert!(result.is_some());
        assert_eq!(result.unwrap().get("type").unwrap(), "object");
    }

    #[test]
    fn collect_properties_flat() {
        let schema = serde_json::json!({
            "properties": {
                "name": { "type": "string", "description": "User name" },
                "age": { "type": "number" }
            },
            "required": ["name"]
        });
        let props = collect_properties(&schema, &[]);
        assert_eq!(props.len(), 2);
        let name_prop = props.iter().find(|p| p.name == "name").unwrap();
        assert!(name_prop.required);
        assert_eq!(name_prop.schema_type.as_deref(), Some("string"));
        assert_eq!(name_prop.description.as_deref(), Some("User name"));
        let age_prop = props.iter().find(|p| p.name == "age").unwrap();
        assert!(!age_prop.required);
    }

    #[test]
    fn collect_properties_allof_merge() {
        let schema = serde_json::json!({
            "allOf": [
                {
                    "properties": {
                        "name": { "type": "string" }
                    },
                    "required": ["name"]
                },
                {
                    "properties": {
                        "age": { "type": "number" }
                    }
                }
            ]
        });
        let props = collect_properties(&schema, &[]);
        assert_eq!(props.len(), 2);
        let name_prop = props.iter().find(|p| p.name == "name").unwrap();
        assert!(name_prop.required);
    }

    #[test]
    fn collect_properties_with_ref() {
        let schema = serde_json::json!({
            "$defs": {
                "Address": {
                    "properties": {
                        "street": { "type": "string", "title": "Street" }
                    }
                }
            },
            "properties": {
                "home": { "$ref": "#/$defs/Address" }
            }
        });
        let props = collect_properties(&schema, &["home".into()]);
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].name, "street");
    }

    #[test]
    fn collect_properties_nested() {
        let schema = serde_json::json!({
            "properties": {
                "server": {
                    "properties": {
                        "host": { "type": "string" },
                        "port": { "type": "number" }
                    }
                }
            }
        });
        let props = collect_properties(&schema, &["server".into()]);
        assert_eq!(props.len(), 2);
    }

    #[test]
    fn collect_properties_empty_for_non_object() {
        let schema = serde_json::json!({ "type": "string" });
        let props = collect_properties(&schema, &[]);
        assert!(props.is_empty());
    }

    #[test]
    fn collect_values_enum() {
        let schema = serde_json::json!({
            "properties": {
                "mode": {
                    "enum": ["dark", "light", "auto"]
                }
            }
        });
        let values = collect_values(&schema, &[], "mode");
        assert_eq!(values.len(), 3);
        assert!(matches!(&values[0], ValueSuggestion::Enum(v) if v == "dark"));
    }

    #[test]
    fn collect_values_const() {
        let schema = serde_json::json!({
            "properties": {
                "version": { "const": 2 }
            }
        });
        let values = collect_values(&schema, &[], "version");
        assert_eq!(values.len(), 1);
        assert!(matches!(&values[0], ValueSuggestion::Const(v) if v == &serde_json::json!(2)));
    }

    #[test]
    fn collect_values_boolean() {
        let schema = serde_json::json!({
            "properties": {
                "enabled": { "type": "boolean" }
            }
        });
        let values = collect_values(&schema, &[], "enabled");
        assert_eq!(values.len(), 1);
        assert!(matches!(&values[0], ValueSuggestion::Boolean));
    }

    #[test]
    fn collect_values_no_suggestions() {
        let schema = serde_json::json!({
            "properties": {
                "name": { "type": "string" }
            }
        });
        let values = collect_values(&schema, &[], "name");
        assert!(values.is_empty());
    }

    #[test]
    fn hover_truncate_multibyte_boundary() {
        // Place a 4-byte emoji (U+1F600) right at the 10,000-byte boundary.
        let prefix = "x".repeat(9_999);
        let long_desc = format!("{prefix}\u{1F600}extra");

        let schema = serde_json::json!({
            "properties": {
                "x": { "description": long_desc }
            }
        });

        // Must not panic, and must produce valid truncated output.
        let result = lookup_hover_content(&schema, &["x".into()]).unwrap();
        assert!(result.ends_with("..."));
        // The emoji straddles byte 10,000, so it should be excluded.
        assert!(!result.contains('\u{1F600}'));
    }
}

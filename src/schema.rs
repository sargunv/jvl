use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use thiserror::Error;

use crate::diagnostic::Warning;

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

/// Where the schema came from.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum SchemaSource {
    /// A local file path (absolute).
    File(PathBuf),
    /// An HTTP/HTTPS URL.
    Url(String),
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
        SchemaSource::File(abs)
    }
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
    warnings: Vec<Warning>,
    cache_outcome: Option<CacheOutcome>,
}

impl SchemaCache {
    pub fn new() -> Self {
        Self::default()
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
            let mut slots = self.slots.lock().unwrap();
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
                        warnings,
                        cache_outcome,
                    };
                }
            };

            let validator = match jsonschema::options()
                .with_retriever(CachingRetriever { no_cache })
                .build(&schema_value)
            {
                Ok(v) => v,
                Err(e) => {
                    return SlotResult {
                        validator: Err(SchemaError::CompileError(e.to_string())),
                        warnings,
                        cache_outcome,
                    };
                }
            };

            SlotResult {
                validator: Ok(Arc::new(validator)),
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

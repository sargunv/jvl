mod common;

use common::jvl;

/// Helper: set up a temp project with a subdirectory structure:
///   project/
///     jvl.json          (maps src/**/*.json -> schema.json)
///     schemas/
///       schema.json     (requires name: string, port: number)
///     src/
///       invalid.json    (port is a string — should fail validation)
///       valid.json      (passes validation)
fn setup_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();

    std::fs::create_dir_all(dir.path().join("schemas")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();

    std::fs::write(
        dir.path().join("schemas/schema.json"),
        r#"{
  "type": "object",
  "properties": {
    "name": { "type": "string" },
    "port": { "type": "number" }
  },
  "required": ["name", "port"]
}"#,
    )
    .unwrap();

    // Schema mapping uses a path-specific glob (src/**/*.json)
    std::fs::write(
        dir.path().join("jvl.json"),
        r#"{
  "files": ["src/**/*.json"],
  "schemas": [
    { "path": "schemas/schema.json", "files": ["src/**/*.json"] }
  ]
}"#,
    )
    .unwrap();

    std::fs::write(
        dir.path().join("src/invalid.json"),
        r#"{ "name": "app", "port": "not-a-number" }"#,
    )
    .unwrap();

    std::fs::write(
        dir.path().join("src/valid.json"),
        r#"{ "name": "app", "port": 8080 }"#,
    )
    .unwrap();

    dir
}

fn parse_json_output(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON: {e}\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Baseline: auto-discovery from project root applies schema mappings.
#[test]
fn schema_mapping_works_with_auto_discovery() {
    let dir = setup_project();

    let output = jvl()
        .args(["check", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    let json = parse_json_output(&output);

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(2),
        "Both src/ files should be checked via schema mapping\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["invalid_files"].as_u64(),
        Some(1),
        "invalid.json should fail validation\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["skipped_files"].as_u64(),
        Some(0),
        "No files should be skipped\njson: {json:#}"
    );
}

/// Passing a relative path from project root should apply schema mappings.
/// The file path is "src/invalid.json" which should match "src/**/*.json".
///
/// Bug: strip_prefix(absolute_project_root) on relative path "src/invalid.json"
/// fails, so the raw path is used for glob matching. This happens to work for
/// this case because "src/invalid.json" matches "src/**/*.json" as a string.
/// See the subdirectory test below for where it actually breaks.
#[test]
fn schema_mapping_works_with_relative_path_from_project_root() {
    let dir = setup_project();

    let output = jvl()
        .args(["check", "--format", "json", "src/invalid.json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    let json = parse_json_output(&output);

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(1),
        "File should be checked with schema mapping applied\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["skipped_files"].as_u64(),
        Some(0),
        "File should not be skipped\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["invalid_files"].as_u64(),
        Some(1),
        "File should fail schema validation\njson: {json:#}"
    );
}

/// Passing an absolute path should apply schema mappings.
#[test]
fn schema_mapping_works_with_absolute_path() {
    let dir = setup_project();
    let abs = dir.path().join("src/invalid.json");

    let output = jvl()
        .args(["check", "--format", "json", abs.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    let json = parse_json_output(&output);

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(1),
        "File should be checked with schema mapping applied\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["skipped_files"].as_u64(),
        Some(0),
        "File should not be skipped\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["invalid_files"].as_u64(),
        Some(1),
        "File should fail schema validation\njson: {json:#}"
    );
}

/// Bug: running from a subdirectory and passing a relative path breaks
/// schema mapping.
///
/// cwd = project/src, file arg = "invalid.json"
/// project_root = project/ (found via upward config walk)
/// path_str = "invalid.json" (as displayed)
/// strip_prefix("project/") on "invalid.json" → Err → fallback = "invalid.json"
/// glob match "src/**/*.json" against "invalid.json" → no match
/// → file gets skipped instead of validated
#[test]
fn schema_mapping_works_with_relative_path_from_subdirectory() {
    let dir = setup_project();

    let output = jvl()
        .args(["check", "--format", "json", "invalid.json"])
        .current_dir(dir.path().join("src"))
        .output()
        .expect("failed to run jvl");

    let json = parse_json_output(&output);

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(1),
        "File should be checked with schema mapping applied\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["skipped_files"].as_u64(),
        Some(0),
        "File should not be skipped\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["invalid_files"].as_u64(),
        Some(1),
        "File should fail schema validation\njson: {json:#}"
    );
}

/// Same bug with dotslash path from subdirectory.
#[test]
fn schema_mapping_works_with_dotslash_path_from_subdirectory() {
    let dir = setup_project();

    let output = jvl()
        .args(["check", "--format", "json", "./invalid.json"])
        .current_dir(dir.path().join("src"))
        .output()
        .expect("failed to run jvl");

    let json = parse_json_output(&output);

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(1),
        "File should be checked with schema mapping applied\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["skipped_files"].as_u64(),
        Some(0),
        "File should not be skipped\njson: {json:#}"
    );
}

/// Bug with parent-traversal path from subdirectory.
/// cwd = project/src, file arg = "../src/invalid.json"
/// The file resolves correctly for reading, but schema mapping fails.
#[test]
fn schema_mapping_works_with_parent_traversal_path() {
    let dir = setup_project();

    let output = jvl()
        .args(["check", "--format", "json", "../src/invalid.json"])
        .current_dir(dir.path().join("src"))
        .output()
        .expect("failed to run jvl");

    let json = parse_json_output(&output);

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(1),
        "File should be checked with schema mapping applied\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["skipped_files"].as_u64(),
        Some(0),
        "File should not be skipped\njson: {json:#}"
    );
}

/// Regression: --config jvl.json (bare filename) with explicit file arg.
/// path.parent() on "jvl.json" returns Some(""), making project_root empty.
/// This broke schema mapping because canonicalize("") fails.
#[test]
fn schema_mapping_works_with_bare_config_path_and_explicit_file() {
    let dir = setup_project();

    let output = jvl()
        .args([
            "check",
            "--format",
            "json",
            "--config",
            "jvl.json",
            "src/invalid.json",
        ])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    let json = parse_json_output(&output);

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(1),
        "File should be checked with schema mapping applied\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["skipped_files"].as_u64(),
        Some(0),
        "File should not be skipped\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["invalid_files"].as_u64(),
        Some(1),
        "File should fail schema validation\njson: {json:#}"
    );
}

/// Same regression with dotslash config path.
#[test]
fn schema_mapping_works_with_dotslash_config_path_and_explicit_file() {
    let dir = setup_project();

    let output = jvl()
        .args([
            "check",
            "--format",
            "json",
            "--config",
            "./jvl.json",
            "src/invalid.json",
        ])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    let json = parse_json_output(&output);

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(1),
        "File should be checked with schema mapping applied\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["skipped_files"].as_u64(),
        Some(0),
        "File should not be skipped\njson: {json:#}"
    );
}

/// --config with bare filename and auto-discovery (no explicit files).
#[test]
fn schema_mapping_works_with_bare_config_path_and_discovery() {
    let dir = setup_project();

    let output = jvl()
        .args(["check", "--format", "json", "--config", "jvl.json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    let json = parse_json_output(&output);

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(2),
        "Both src/ files should be checked via schema mapping\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["invalid_files"].as_u64(),
        Some(1),
        "invalid.json should fail validation\njson: {json:#}"
    );
}

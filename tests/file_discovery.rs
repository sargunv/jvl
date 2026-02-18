mod common;

use common::jvl;

#[test]
fn ordered_include_exclude_patterns() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("vendor")).unwrap();

    std::fs::write(
        dir.path().join("jvl.json"),
        r#"{"files": ["**/*.json", "!vendor/**", "vendor/allow.json"]}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("top.json"), "{}").unwrap();
    std::fs::write(dir.path().join("vendor/blocked.json"), "{}").unwrap();
    std::fs::write(dir.path().join("vendor/allow.json"), "{}").unwrap();

    let output = jvl()
        .args(["check", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    let checked = json["summary"]["checked_files"].as_u64().unwrap()
        + json["summary"]["skipped_files"].as_u64().unwrap();

    let paths: Vec<&str> = json["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect();
    let all_paths_str = format!("{paths:?}");

    assert!(
        !all_paths_str.contains("blocked"),
        "vendor/blocked.json should be excluded\n{all_paths_str}"
    );
    assert!(
        checked >= 2,
        "Should discover at least top.json + vendor/allow.json\n{all_paths_str}"
    );
}

#[test]
fn discovery_from_subdirectory_scopes_to_cwd() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("other")).unwrap();

    std::fs::write(dir.path().join("jvl.json"), r#"{"files": ["**/*.json"]}"#).unwrap();
    std::fs::write(dir.path().join("src/a.json"), "{}").unwrap();
    std::fs::write(dir.path().join("other/b.json"), "{}").unwrap();

    let output = jvl()
        .args(["check", "--format", "json"])
        .current_dir(dir.path().join("src"))
        .output()
        .expect("failed to run jvl");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    let total = json["summary"]["checked_files"].as_u64().unwrap()
        + json["summary"]["skipped_files"].as_u64().unwrap();
    assert_eq!(
        total, 1,
        "Should only discover files under src/\njson: {json:#}"
    );
}

#[test]
fn directory_argument_expands_to_contained_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("other")).unwrap();

    std::fs::write(dir.path().join("jvl.json"), r#"{"files": ["**/*.json"]}"#).unwrap();
    std::fs::write(dir.path().join("src/a.json"), "{}").unwrap();
    std::fs::write(dir.path().join("other/b.json"), "{}").unwrap();

    let output = jvl()
        .args(["check", "--format", "json", "src"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    let total = json["summary"]["checked_files"].as_u64().unwrap()
        + json["summary"]["skipped_files"].as_u64().unwrap();
    assert_eq!(
        total, 1,
        "Should only discover files under src/\njson: {json:#}"
    );
}

#[test]
fn mixed_directory_and_file_arguments() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();

    std::fs::write(dir.path().join("jvl.json"), r#"{"files": ["**/*.json"]}"#).unwrap();
    std::fs::write(dir.path().join("src/a.json"), "{}").unwrap();
    std::fs::write(dir.path().join("top.json"), "{}").unwrap();

    let output = jvl()
        .args(["check", "--format", "json", "src", "top.json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    let total = json["summary"]["checked_files"].as_u64().unwrap()
        + json["summary"]["skipped_files"].as_u64().unwrap();
    assert_eq!(
        total, 2,
        "Should discover src/a.json from directory + top.json from file arg\njson: {json:#}"
    );
}

#[test]
fn directory_argument_applies_schema_mappings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("schemas")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();

    std::fs::write(
        dir.path().join("schemas/schema.json"),
        r#"{
  "type": "object",
  "properties": { "name": { "type": "string" } },
  "required": ["name"]
}"#,
    )
    .unwrap();

    std::fs::write(
        dir.path().join("jvl.json"),
        r#"{
  "files": ["src/**/*.json"],
  "schemas": [{ "path": "schemas/schema.json", "files": ["src/**/*.json"] }]
}"#,
    )
    .unwrap();

    std::fs::write(dir.path().join("src/valid.json"), r#"{ "name": "ok" }"#).unwrap();
    std::fs::write(dir.path().join("src/invalid.json"), r#"{ "name": 123 }"#).unwrap();

    let output = jvl()
        .args(["check", "--format", "json", "src"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(2),
        "Both files should be checked\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["invalid_files"].as_u64(),
        Some(1),
        "invalid.json should fail\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["skipped_files"].as_u64(),
        Some(0),
        "No files should be skipped\njson: {json:#}"
    );
}

#[test]
fn subdirectory_discovery_applies_schema_mappings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("schemas")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();

    std::fs::write(
        dir.path().join("schemas/schema.json"),
        r#"{
  "type": "object",
  "properties": { "name": { "type": "string" } },
  "required": ["name"]
}"#,
    )
    .unwrap();

    std::fs::write(
        dir.path().join("jvl.json"),
        r#"{
  "files": ["src/**/*.json"],
  "schemas": [{ "path": "schemas/schema.json", "files": ["src/**/*.json"] }]
}"#,
    )
    .unwrap();

    std::fs::write(dir.path().join("src/invalid.json"), r#"{ "name": 123 }"#).unwrap();

    let output = jvl()
        .args(["check", "--format", "json"])
        .current_dir(dir.path().join("src"))
        .output()
        .expect("failed to run jvl");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");

    assert_eq!(
        json["summary"]["checked_files"].as_u64(),
        Some(1),
        "File should be checked with schema mapping\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["skipped_files"].as_u64(),
        Some(0),
        "File should not be skipped\njson: {json:#}"
    );
    assert_eq!(
        json["summary"]["invalid_files"].as_u64(),
        Some(1),
        "File should fail validation\njson: {json:#}"
    );
}

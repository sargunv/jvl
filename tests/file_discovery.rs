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

mod common;

use common::{fixture, jvl};

#[test]
fn valid_file_with_explicit_schema() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            &fixture("valid.json"),
        ])
        .output()
        .expect("failed to run jvl");

    assert!(
        output.status.success(),
        "Expected exit code 0, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn invalid_type_error() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            &fixture("invalid-type.json"),
        ])
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn missing_required_property() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            &fixture("missing-required.json"),
        ])
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn jsonc_with_comments() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            &fixture("with-comments.jsonc"),
        ])
        .output()
        .expect("failed to run jvl");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn no_schema_skips_by_default() {
    let output = jvl()
        .args(["check", &fixture("no-schema.json")])
        .output()
        .expect("failed to run jvl");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn strict_mode_errors_on_no_schema() {
    let output = jvl()
        .args(["check", "--strict", &fixture("no-schema.json")])
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn schema_from_dollar_schema_field() {
    let output = jvl()
        .args(["check", &fixture("with-schema-field.json")])
        .output()
        .expect("failed to run jvl");

    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn help_flag() {
    let output = jvl().args(["--help"]).output().expect("failed to run jvl");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("JSON Schema Validator"));
}

#[test]
fn version_flag() {
    let output = jvl()
        .args(["--version"])
        .output()
        .expect("failed to run jvl");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("jvl"));
}

#[test]
fn multiple_files() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            &fixture("valid.json"),
            &fixture("invalid-type.json"),
        ])
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn empty_file_with_schema() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            &fixture("empty.json"),
        ])
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn only_comments_file_with_schema() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            &fixture("only-comments.jsonc"),
        ])
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn deeply_nested_error() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("deeply-nested-schema.json"),
            &fixture("deeply-nested-invalid.json"),
        ])
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn syntax_error_file() {
    use std::io::Write;
    let mut tmp = tempfile::Builder::new()
        .suffix(".json")
        .tempfile()
        .expect("tempfile");
    // Unquoted `broken` is a JSON syntax error (identifiers are not values).
    tmp.write_all(b"{ \"name\": broken, \"port\": 8080 }")
        .expect("write");

    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn exit_code_2_for_missing_schema_file() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            "/nonexistent/schema.json",
            &fixture("valid.json"),
        ])
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn exit_code_2_for_bad_config() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("jvl.json"), "{ invalid json }").unwrap();

    let output = jvl()
        .args([
            "check",
            "--config",
            &dir.path().join("jvl.json").display().to_string(),
            &fixture("valid.json"),
        ])
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn exit_code_2_for_auto_discovered_bad_config() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("jvl.json"), "{ invalid json }").unwrap();
    std::fs::write(dir.path().join("test.json"), "{}").unwrap();

    let output = jvl()
        .args(["check"])
        .current_dir(dir.path())
        .output()
        .expect("failed to run jvl");

    assert_eq!(output.status.code(), Some(2));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to load config"), "stderr: {stderr}");
}

#[test]
fn bom_handling() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            &fixture("with-bom.json"),
        ])
        .output()
        .expect("failed to run jvl");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

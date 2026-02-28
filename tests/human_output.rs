mod common;

use common::{fixture, jvl, jvl_human, with_human_settings, with_human_settings_extra};

// ── Existing human output tests ───────────────────────────────────────

#[test]
fn summary_line_valid() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            &fixture("valid.json"),
        ])
        .output()
        .expect("failed to run jvl");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("All") && stderr.contains("valid"),
        "stderr: {stderr}"
    );
}

#[test]
fn summary_line_errors() {
    let output = jvl()
        .args([
            "check",
            "--schema",
            &fixture("simple-schema.json"),
            &fixture("invalid-type.json"),
        ])
        .output()
        .expect("failed to run jvl");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Found") && stderr.contains("error"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("Checked"), "stderr: {stderr}");
}

#[test]
fn bom_diagnostic_alignment() {
    use std::io::Write;
    let mut tmp = tempfile::Builder::new()
        .suffix(".json")
        .tempfile()
        .expect("tempfile");
    // UTF-8 BOM followed by multi-line JSON. The invalid "port" value lands on
    // line 3, column 11. Without BOM stripping the column would be wrong.
    tmp.write_all(b"\xEF\xBB\xBF{\n  \"name\": \"my-app\",\n  \"port\": \"not-a-number\"\n}\n")
        .expect("write");

    let path = tmp.path().to_str().unwrap().to_owned();
    let (stderr, code) = jvl_human(&["check", "--schema", &fixture("simple-schema.json"), &path]);
    assert_eq!(code, 1);

    with_human_settings_extra(&[(&path, "[fixtures]/with-bom-invalid.json")], || {
        insta::assert_snapshot!(stderr);
    });
}

// ── Per-keyword human output snapshot tests ───────────────────────────

#[test]
fn type_mismatch() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("simple-schema.json"),
        &fixture("invalid-type.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn required() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("simple-schema.json"),
        &fixture("missing-required.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn enum_mismatch() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("enum-schema.json"),
        &fixture("enum-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn const_mismatch() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("const-schema.json"),
        &fixture("const-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn pattern() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("pattern-schema.json"),
        &fixture("pattern-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn additional_properties() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("additionalProperties-schema.json"),
        &fixture("additionalProperties-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn additional_properties_multiple() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("additionalProperties-schema.json"),
        &fixture("additionalProperties-multiple-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn additional_items() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("additionalItems-schema.json"),
        &fixture("additionalItems-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn unevaluated_properties() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("unevaluatedProperties-schema.json"),
        &fixture("unevaluatedProperties-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn minimum() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("minimum-schema.json"),
        &fixture("minimum-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn maximum() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("maximum-schema.json"),
        &fixture("maximum-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn exclusive_minimum() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("exclusiveMinimum-schema.json"),
        &fixture("exclusiveMinimum-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn exclusive_maximum() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("exclusiveMaximum-schema.json"),
        &fixture("exclusiveMaximum-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn min_length() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("minLength-schema.json"),
        &fixture("minLength-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn max_length() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("maxLength-schema.json"),
        &fixture("maxLength-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn min_items() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("minItems-schema.json"),
        &fixture("minItems-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn max_items() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("maxItems-schema.json"),
        &fixture("maxItems-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn min_properties() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("minProperties-schema.json"),
        &fixture("minProperties-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn max_properties() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("maxProperties-schema.json"),
        &fixture("maxProperties-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn multiple_of() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("multipleOf-schema.json"),
        &fixture("multipleOf-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn unique_items() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("uniqueItems-schema.json"),
        &fixture("uniqueItems-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn any_of() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("anyOf-schema.json"),
        &fixture("anyOf-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn one_of_not_valid() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("oneOf-notValid-schema.json"),
        &fixture("oneOf-notValid-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn one_of_multiple_valid() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("oneOf-multipleValid-schema.json"),
        &fixture("oneOf-multipleValid-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn not_schema() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("not-schema.json"),
        &fixture("not-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn contains() {
    let (stderr, code) = jvl_human(&[
        "check",
        "--schema",
        &fixture("contains-schema.json"),
        &fixture("contains-invalid.json"),
    ]);
    assert_eq!(code, 1);
    with_human_settings(|| insta::assert_snapshot!(stderr));
}

#[test]
fn schema_load_error_points_at_schema_value() {
    let (stderr, code) = jvl_human(&["check", &fixture("schema-load-error.json")]);
    assert_eq!(code, 2);
    // The underline should point at the $schema value, not at (0,0).
    assert!(
        stderr.contains("schema referenced here"),
        "expected 'schema referenced here' label in stderr: {stderr}"
    );
    // The source header should point at the $schema value (col 14), not (1,1).
    assert!(
        stderr.contains(":1:14]"),
        "expected source location :1:14 in stderr: {stderr}"
    );
}

#[test]
fn schema_compile_error_points_at_schema_value() {
    let (stderr, code) = jvl_human(&["check", &fixture("schema-compile-error.json")]);
    assert_eq!(code, 2);
    assert!(
        stderr.contains("schema referenced here"),
        "expected 'schema referenced here' label in stderr: {stderr}"
    );
    assert!(
        stderr.contains(":1:14]"),
        "expected source location :1:14 in stderr: {stderr}"
    );
}

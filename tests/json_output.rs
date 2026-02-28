mod common;

use common::{fixture, jvl_json};

#[test]
fn valid_file() {
    let (json, code) = jvl_json(&[
        "check",
        "--format",
        "json",
        "--schema",
        &fixture("simple-schema.json"),
        &fixture("valid.json"),
    ]);

    assert_eq!(code, 0);
    insta::assert_json_snapshot!(json, {
        ".files[].path" => "[path]",
        ".summary.duration_ms" => "[duration]",
    }, @r#"
    {
      "files": [
        {
          "errors": [],
          "path": "[path]",
          "valid": true
        }
      ],
      "summary": {
        "checked_files": 1,
        "duration_ms": "[duration]",
        "errors": 0,
        "invalid_files": 0,
        "skipped_files": 0,
        "valid_files": 1,
        "warnings": 0
      },
      "valid": true,
      "version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn type_error() {
    let (json, code) = jvl_json(&[
        "check",
        "--format",
        "json",
        "--schema",
        &fixture("simple-schema.json"),
        &fixture("invalid-type.json"),
    ]);

    assert_eq!(code, 1);
    insta::assert_json_snapshot!(json, {
        ".files[].path" => "[path]",
        ".summary.duration_ms" => "[duration]",
    }, @r#"
    {
      "files": [
        {
          "errors": [
            {
              "code": "schema(type)",
              "location": {
                "column": 29,
                "length": 14,
                "line": 1,
                "offset": 28
              },
              "message": "\"not-a-number\" is not of type \"number\"",
              "schema_path": "/properties/port/type",
              "severity": "error"
            }
          ],
          "path": "[path]",
          "valid": false
        }
      ],
      "summary": {
        "checked_files": 1,
        "duration_ms": "[duration]",
        "errors": 1,
        "invalid_files": 1,
        "skipped_files": 0,
        "valid_files": 0,
        "warnings": 0
      },
      "valid": false,
      "version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn missing_required() {
    let (json, code) = jvl_json(&[
        "check",
        "--format",
        "json",
        "--schema",
        &fixture("simple-schema.json"),
        &fixture("missing-required.json"),
    ]);

    assert_eq!(code, 1);
    insta::assert_json_snapshot!(json, {
        ".files[].path" => "[path]",
        ".summary.duration_ms" => "[duration]",
    }, @r#"
    {
      "files": [
        {
          "errors": [
            {
              "code": "schema(required)",
              "location": {
                "column": 1,
                "length": 19,
                "line": 1,
                "offset": 0
              },
              "message": "\"name\" is a required property",
              "schema_path": "/required",
              "severity": "error"
            },
            {
              "code": "schema(required)",
              "location": {
                "column": 1,
                "length": 19,
                "line": 1,
                "offset": 0
              },
              "message": "\"port\" is a required property",
              "schema_path": "/required",
              "severity": "error"
            }
          ],
          "path": "[path]",
          "valid": false
        }
      ],
      "summary": {
        "checked_files": 1,
        "duration_ms": "[duration]",
        "errors": 2,
        "invalid_files": 1,
        "skipped_files": 0,
        "valid_files": 0,
        "warnings": 0
      },
      "valid": false,
      "version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn syntax_error() {
    use std::io::Write;
    let mut tmp = tempfile::Builder::new()
        .suffix(".json")
        .tempfile()
        .expect("tempfile");
    // Multi-line JSON with a missing comma: produces "Expected comma on line 2
    // column 19" regardless of how a formatter would rewrite the content.
    tmp.write_all(b"{\n  \"name\": \"broken\"\n  \"port\": 8080\n}\n")
        .expect("write");

    let (json, code) = jvl_json(&[
        "check",
        "--format",
        "json",
        "--schema",
        &fixture("simple-schema.json"),
        tmp.path().to_str().unwrap(),
    ]);

    assert_eq!(code, 1);
    insta::assert_json_snapshot!(json, {
        ".files[].path" => "[path]",
        ".summary.duration_ms" => "[duration]",
    }, @r#"
    {
      "files": [
        {
          "errors": [
            {
              "code": "parse(syntax)",
              "location": {
                "column": 19,
                "length": 0,
                "line": 2,
                "offset": 20
              },
              "message": "Expected comma on line 2 column 19",
              "severity": "error"
            }
          ],
          "path": "[path]",
          "valid": false
        }
      ],
      "summary": {
        "checked_files": 1,
        "duration_ms": "[duration]",
        "errors": 1,
        "invalid_files": 1,
        "skipped_files": 0,
        "valid_files": 0,
        "warnings": 0
      },
      "valid": false,
      "version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn skipped_file() {
    let (json, code) = jvl_json(&["check", "--format", "json", &fixture("no-schema.json")]);

    assert_eq!(code, 0);
    insta::assert_json_snapshot!(json, {
        ".summary.duration_ms" => "[duration]",
    }, @r#"
    {
      "files": [],
      "summary": {
        "checked_files": 0,
        "duration_ms": "[duration]",
        "errors": 0,
        "invalid_files": 0,
        "skipped_files": 1,
        "valid_files": 0,
        "warnings": 0
      },
      "valid": true,
      "version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn strict_no_schema() {
    let (json, code) = jvl_json(&[
        "check",
        "--strict",
        "--format",
        "json",
        &fixture("no-schema.json"),
    ]);

    assert_eq!(code, 1);
    insta::assert_json_snapshot!(json, {
        ".files[].path" => "[path]",
        ".summary.duration_ms" => "[duration]",
    }, @r#"
    {
      "files": [
        {
          "errors": [
            {
              "code": "no-schema",
              "message": "no schema found",
              "severity": "error"
            }
          ],
          "path": "[path]",
          "valid": false
        }
      ],
      "summary": {
        "checked_files": 1,
        "duration_ms": "[duration]",
        "errors": 1,
        "invalid_files": 1,
        "skipped_files": 0,
        "valid_files": 0,
        "warnings": 0
      },
      "valid": false,
      "version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn tool_error() {
    let (json, code) = jvl_json(&[
        "check",
        "--format",
        "json",
        "--schema",
        "/nonexistent/schema.json",
        &fixture("valid.json"),
    ]);

    assert_eq!(code, 2);
    insta::assert_json_snapshot!(json, {
        ".files[].path" => "[path]",
        ".summary.duration_ms" => "[duration]",
    }, @r#"
    {
      "files": [
        {
          "errors": [
            {
              "code": "schema(load)",
              "message": "Failed to read schema file '/nonexistent/schema.json': No such file or directory (os error 2)",
              "severity": "error"
            }
          ],
          "path": "[path]",
          "valid": false
        }
      ],
      "summary": {
        "checked_files": 1,
        "duration_ms": "[duration]",
        "errors": 1,
        "invalid_files": 1,
        "skipped_files": 0,
        "valid_files": 0,
        "warnings": 0
      },
      "valid": false,
      "version": 1,
      "warnings": []
    }
    "#);
}

#[test]
fn schema_load_error_with_schema_field() {
    let (json, code) = jvl_json(&[
        "check",
        "--format",
        "json",
        &fixture("schema-load-error.json"),
    ]);

    assert_eq!(code, 2);
    // Verify the error has a location pointing at the $schema value span.
    let errors = json["files"][0]["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    let error = &errors[0];
    assert_eq!(error["code"], "schema(load)");
    // location should exist and point at the $schema value (line 1, column 14)
    let loc = &error["location"];
    assert_eq!(loc["line"], 1, "expected line 1");
    assert_eq!(loc["column"], 14, "expected column 14");
    assert!(
        loc["length"].as_u64().unwrap() > 0,
        "expected nonzero length"
    );
}

#[test]
fn schema_load_error_with_flag_no_location() {
    // When schema comes from --schema flag, there's no $schema field to point at.
    let (json, code) = jvl_json(&[
        "check",
        "--format",
        "json",
        "--schema",
        "/nonexistent/schema.json",
        &fixture("valid.json"),
    ]);

    assert_eq!(code, 2);
    let errors = json["files"][0]["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    let error = &errors[0];
    assert_eq!(error["code"], "schema(load)");
    // No location when schema comes from --schema flag.
    assert!(
        error.get("location").is_none() || error["location"].is_null(),
        "expected no location for --schema flag errors"
    );
}

#[test]
fn schema_compile_error_with_schema_field() {
    let (json, code) = jvl_json(&[
        "check",
        "--format",
        "json",
        &fixture("schema-compile-error.json"),
    ]);

    assert_eq!(code, 2);
    let errors = json["files"][0]["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    let error = &errors[0];
    assert_eq!(error["code"], "schema(compile)");
    // location should exist and point at the $schema value
    let loc = &error["location"];
    assert_eq!(loc["line"], 1, "expected line 1");
    assert_eq!(loc["column"], 14, "expected column 14");
    assert!(
        loc["length"].as_u64().unwrap() > 0,
        "expected nonzero length"
    );
}

#[test]
fn deeply_nested_error() {
    let (json, code) = jvl_json(&[
        "check",
        "--format",
        "json",
        "--schema",
        &fixture("deeply-nested-schema.json"),
        &fixture("deeply-nested-invalid.json"),
    ]);

    assert_eq!(code, 1);
    insta::assert_json_snapshot!(json, {
        ".files[].path" => "[path]",
        ".summary.duration_ms" => "[duration]",
    }, @r#"
    {
      "files": [
        {
          "errors": [
            {
              "code": "schema(type)",
              "location": {
                "column": 48,
                "length": 14,
                "line": 1,
                "offset": 47
              },
              "message": "\"not-a-number\" is not of type \"number\"",
              "schema_path": "/properties/level1/properties/level2/properties/level3/properties/value/type",
              "severity": "error"
            }
          ],
          "path": "[path]",
          "valid": false
        }
      ],
      "summary": {
        "checked_files": 1,
        "duration_ms": "[duration]",
        "errors": 1,
        "invalid_files": 1,
        "skipped_files": 0,
        "valid_files": 0,
        "warnings": 0
      },
      "valid": false,
      "version": 1,
      "warnings": []
    }
    "#);
}

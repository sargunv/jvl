use std::process::Command;

pub fn jvl() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jvl"))
}

#[allow(dead_code)]
pub fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Run jvl with --format json and parse the output.
#[allow(dead_code)]
pub fn jvl_json(args: &[&str]) -> (serde_json::Value, i32) {
    let output = jvl().args(args).output().expect("failed to run jvl");
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON: {e}\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (json, code)
}

/// Run jvl with NO_COLOR=1 and return (stderr, exit_code).
#[allow(dead_code)]
pub fn jvl_human(args: &[&str]) -> (String, i32) {
    let output = jvl()
        .env("NO_COLOR", "1")
        .args(args)
        .output()
        .expect("failed to run jvl");
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (stderr, code)
}

/// Apply insta settings that redact absolute fixture paths and durations in human output.
#[allow(dead_code)]
pub fn with_human_settings(f: impl FnOnce()) {
    with_human_settings_extra(&[], f);
}

/// Like [`with_human_settings`] but also redacts additional literal paths.
/// Each entry in `extra` is `(path_to_redact, replacement_alias)`.
#[allow(dead_code)]
pub fn with_human_settings_extra(extra: &[(&str, &str)], f: impl FnOnce()) {
    let mut settings = insta::Settings::clone_current();
    // Redact absolute path to fixtures directory
    let fixtures_dir = format!("{}/tests/fixtures/", env!("CARGO_MANIFEST_DIR"));
    settings.add_filter(&regex_escape(&fixtures_dir), "[fixtures]/");
    // Redact duration in summary lines like "(123ms)" or "(1.2s)" or "(10s)"
    settings.add_filter(r"\(\d+(?:\.\d+)?m?s\)", "([duration])");
    for (path, alias) in extra {
        settings.add_filter(&regex_escape(path), *alias);
    }
    settings.bind(f);
}

#[allow(dead_code)]
fn regex_escape(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}

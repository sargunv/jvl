# jvl

Validate JSON and JSONC files against [JSON Schema](https://json-schema.org/).
jvl automatically respects `$schema` fields and supports a project-level config
file for mapping schemas to file patterns.

## Installation

Install with [mise](https://mise.jdx.dev/):

```sh
mise use github:sargunv/jvl
```

Or download a binary from
[GitHub Releases](https://github.com/sargunv/jvl/releases).

## How it works

jvl resolves which schema to use for each file in this order:

1. `--schema` flag: override the schema for all files
2. `$schema` field in the JSON file
3. Config mapping in `jvl.json`
4. Skip the file (or error with `--strict`)

## Usage

```sh
# Validate all discovered files
jvl check

# Validate specific files
jvl check config.json data/*.json

# Override the schema for all files
jvl check --schema schema.json data/*.json

# Error if any file has no resolvable schema
jvl check --strict

# Machine-readable output
jvl check --format json
```

Other options: `--config <path>` (explicit config file), `--jobs <n>`
(parallelism, default 10), `--no-cache` (bypass schema cache).

Sample output:

```
  × schema(type): "wrong-type" is not of type "number"
   ╭─[config.json:1:64]
 1 │ { "$schema": "./schema.json", "name": "my-app", "port": "wrong-type" }
   ·                                                         ──────┬─────
   ·                                                               ╰── expected type "number"
   ╰────

✗ Found 1 error in 1 file
  Checked 1 file (15ms)
```

Generate shell completions:

```sh
jvl completions bash  # or zsh, fish, powershell
```

## Configuration

jvl looks for a `jvl.json` file in the current directory and parent directories.
Example:

```jsonc
{
  "$schema": "https://raw.githubusercontent.com/sargunv/jvl/main/config.schema.json",
  "files": ["**/*.json", "**/*.jsonc", "!package-lock.json"],
  "schemas": [
    {
      "files": ["data/**/*.json"],
      "url": "https://example.com/data-schema.json",
    },
    { "files": ["config/**/*.json"], "path": "schemas/config.schema.json" },
  ],
}
```

- **`files`**: glob patterns for file discovery. Prefix with `!` to exclude.
  Later patterns override earlier ones. Default: `["**/*.json", "**/*.jsonc"]`.
- **`schemas`**: map file patterns to a schema by `url` or local `path`.
- **`$schema`**: optional, enables editor autocompletion for the config itself.

See [`config.schema.json`](config.schema.json) for the full schema reference.

## CI and pre-commit hooks

Exit codes: 0 (all valid), 1 (validation errors), 2 (tool error).

Use `--format json` for machine-readable output.

Example [hk](https://hk.jdx.dev/) config:

```pkl
["jvl"] {
  check = "jvl check --strict"
}
```

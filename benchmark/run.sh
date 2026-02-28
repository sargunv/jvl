#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

FIXTURES="$SCRIPT_DIR/fixtures"
SCHEMAS="$SCRIPT_DIR/schemas"

# Build jvl in release mode
JVL="$ROOT_DIR/target/release/jvl"
echo "Building jvl (release)..."
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml" --quiet

# Download schemas
mkdir -p "$SCHEMAS"

download_schema() {
  local name="$1" url="$2"
  if [[ ! -f "$SCHEMAS/$name" ]]; then
    echo "Downloading $name..."
    curl -sfL "$url" -o "$SCHEMAS/$name"
  fi
}

download_schema "tsconfig.schema.json" "https://json.schemastore.org/tsconfig.json"
download_schema "dprint.schema.json" "https://dprint.dev/schemas/v0.json"
download_schema "biome.schema.json" "https://biomejs.dev/schemas/2.2.6/schema.json"
download_schema "oxlint.schema.json" "https://raw.githubusercontent.com/oxc-project/oxc/main/npm/oxlint/configuration_schema.json"
download_schema "eslintrc.schema.json" "https://json.schemastore.org/eslintrc.json"
download_schema "package.schema.json" "https://json.schemastore.org/package.json"

# ──────────────────────────────────────────────
# Run benchmarks
# ──────────────────────────────────────────────

echo "============================================"
echo " JSON Schema Validator CLI Benchmark"
echo "============================================"
echo ""
echo "Tools:"
echo "  jvl:              $($JVL --version 2>&1)"
echo "  ajv-cli:          $(ajv --help 2>&1 | grep -o 'ajv-cli [0-9.]*' || echo 'ajv-cli 5.0.0')"
echo "  check-jsonschema: $(check-jsonschema --version 2>&1)"
echo "  yajsv:            $(yajsv -v 2>&1)"
echo ""
echo "System: $(uname -ms)"
echo ""

WARMUP=3
RUNS=50

# ──────────────────────────────────────────────
# Benchmark 1: tsconfig.json (draft-04 schema, 79KB schema)
#   ajv-cli does not support draft-04, so it is excluded
# ──────────────────────────────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " Benchmark: tsconfig.json"
echo " Schema: tsconfig (draft-04, 79KB)"
echo " Config: Next.js tsconfig.json (39 lines)"
echo " Note: ajv-cli excluded (no draft-04 support)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
hyperfine \
  --warmup "$WARMUP" \
  --runs "$RUNS" \
  -N \
  --export-markdown "$SCRIPT_DIR/results-tsconfig.md" \
  --command-name "jvl" \
    "$JVL check --schema $SCHEMAS/tsconfig.schema.json $FIXTURES/tsconfig.json" \
  --command-name "check-jsonschema" \
    "check-jsonschema --schemafile $SCHEMAS/tsconfig.schema.json $FIXTURES/tsconfig.json" \
  --command-name "yajsv" \
    "yajsv -s $SCHEMAS/tsconfig.schema.json $FIXTURES/tsconfig.json"

echo ""

# ──────────────────────────────────────────────
# Benchmark 2: dprint.json (draft-07 schema, 3KB schema)
# ──────────────────────────────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " Benchmark: dprint.json"
echo " Schema: dprint (draft-07, 3KB)"
echo " Config: jvl's own dprint.json (20 lines)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
hyperfine \
  --warmup "$WARMUP" \
  --runs "$RUNS" \
  -N \
  --export-markdown "$SCRIPT_DIR/results-dprint.md" \
  --command-name "jvl" \
    "$JVL check --schema $SCHEMAS/dprint.schema.json $FIXTURES/dprint.json" \
  --command-name "ajv-cli" \
    "ajv validate --strict=false -s $SCHEMAS/dprint.schema.json -d $FIXTURES/dprint.json" \
  --command-name "check-jsonschema" \
    "check-jsonschema --schemafile $SCHEMAS/dprint.schema.json $FIXTURES/dprint.json" \
  --command-name "yajsv" \
    "yajsv -s $SCHEMAS/dprint.schema.json $FIXTURES/dprint.json"

echo ""

# ──────────────────────────────────────────────
# Benchmark 3: biome.json (draft-07 schema, 374KB schema)
# ──────────────────────────────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " Benchmark: biome.json"
echo " Schema: biome (draft-07, 374KB)"
echo " Config: biome.json from neuland/micro-frontends (31 lines)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
hyperfine \
  --warmup "$WARMUP" \
  --runs "$RUNS" \
  -N \
  --export-markdown "$SCRIPT_DIR/results-biome.md" \
  --command-name "jvl" \
    "$JVL check --schema $SCHEMAS/biome.schema.json $FIXTURES/biome.json" \
  --command-name "ajv-cli" \
    "ajv validate --strict=false -s $SCHEMAS/biome.schema.json -d $FIXTURES/biome.json" \
  --command-name "check-jsonschema" \
    "check-jsonschema --schemafile $SCHEMAS/biome.schema.json $FIXTURES/biome.json" \
  --command-name "yajsv" \
    "yajsv -s $SCHEMAS/biome.schema.json $FIXTURES/biome.json"

echo ""

# ──────────────────────────────────────────────
# Benchmark 4: oxlint.json (draft-07 schema, 38KB schema)
# ──────────────────────────────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " Benchmark: oxlint.json"
echo " Schema: oxlintrc (draft-07, 38KB)"
echo " Config: oxc's own oxlintrc.json (75 lines)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
hyperfine \
  --warmup "$WARMUP" \
  --runs "$RUNS" \
  -N \
  --export-markdown "$SCRIPT_DIR/results-oxlint.md" \
  --command-name "jvl" \
    "$JVL check --schema $SCHEMAS/oxlint.schema.json $FIXTURES/oxlint.json" \
  --command-name "ajv-cli" \
    "ajv validate --strict=false -s $SCHEMAS/oxlint.schema.json -d $FIXTURES/oxlint.json" \
  --command-name "check-jsonschema" \
    "check-jsonschema --schemafile $SCHEMAS/oxlint.schema.json $FIXTURES/oxlint.json" \
  --command-name "yajsv" \
    "yajsv -s $SCHEMAS/oxlint.schema.json $FIXTURES/oxlint.json"

# ──────────────────────────────────────────────
# Benchmark 5: package.json (draft-07 schema, 44KB schema, external $refs)
#   ajv-cli cannot resolve external $refs, so it is excluded
# ──────────────────────────────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " Benchmark: package.json"
echo " Schema: package.json (draft-07, 44KB, external \$refs)"
echo " Config: ESLint's package.json (182 lines)"
echo " Note: ajv-cli excluded (can't resolve external \$refs)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
hyperfine \
  --warmup "$WARMUP" \
  --runs "$RUNS" \
  -N \
  --export-markdown "$SCRIPT_DIR/results-package.md" \
  --command-name "jvl" \
    "$JVL check --schema $SCHEMAS/package.schema.json $FIXTURES/package.json" \
  --command-name "check-jsonschema" \
    "check-jsonschema --schemafile $SCHEMAS/package.schema.json $FIXTURES/package.json" \
  --command-name "yajsv" \
    "yajsv -s $SCHEMAS/package.schema.json $FIXTURES/package.json"

echo ""
echo "============================================"
echo " Benchmark complete! Results saved to:"
echo "   benchmark/results-*.md"
echo "============================================"

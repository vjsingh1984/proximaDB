#!/usr/bin/env bash
# Track panic-prone code patterns in ProximaDB with staged policy guardrails.
#
# Modes:
#   report          -> metrics only (always exit 0)
#   no-regression   -> fail if total exceeds baseline total
#   module-guard    -> no-regression + fail if critical module totals exceed baseline
#
# Examples:
#   ./scripts/count_panic_patterns.sh
#   ./scripts/count_panic_patterns.sh --mode no-regression
#   ./scripts/count_panic_patterns.sh --mode module-guard \
#     --critical-modules network_rest,api_handlers,graph
#   ./scripts/count_panic_patterns.sh --format json --write /tmp/panic_metrics.json

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SRC_ROOT="$REPO_ROOT/src"
DEFAULT_BASELINE="$REPO_ROOT/docs/_internal/roadmap/PANIC_POLICY_BASELINE.json"

MODE="report"
OUTPUT_FORMAT="text"
BASELINE_PATH=""
WRITE_PATH=""
CRITICAL_MODULES="network_rest,api_handlers"

usage() {
  cat <<'EOF'
Usage: scripts/count_panic_patterns.sh [options]

Options:
  --mode <report|no-regression|module-guard>  Policy mode (default: report)
  --baseline <path>                           Baseline metrics JSON path
  --format <text|json>                        Output format (default: text)
  --write <path>                              Write metrics JSON to file
  --critical-modules <csv>                    Module names for module-guard mode
  -h, --help                                  Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)
      MODE="${2:-}"
      shift 2
      ;;
    --baseline)
      BASELINE_PATH="${2:-}"
      shift 2
      ;;
    --format)
      OUTPUT_FORMAT="${2:-}"
      shift 2
      ;;
    --write)
      WRITE_PATH="${2:-}"
      shift 2
      ;;
    --critical-modules)
      CRITICAL_MODULES="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ "$MODE" != "report" && "$MODE" != "no-regression" && "$MODE" != "module-guard" ]]; then
  echo "Invalid --mode: $MODE" >&2
  exit 2
fi

if [[ "$OUTPUT_FORMAT" != "text" && "$OUTPUT_FORMAT" != "json" ]]; then
  echo "Invalid --format: $OUTPUT_FORMAT" >&2
  exit 2
fi

if [[ ! -d "$SRC_ROOT" ]]; then
  echo "src directory not found at: $SRC_ROOT" >&2
  exit 2
fi

if [[ -z "$BASELINE_PATH" && "$MODE" != "report" && -f "$DEFAULT_BASELINE" ]]; then
  BASELINE_PATH="$DEFAULT_BASELINE"
fi

if [[ -n "$BASELINE_PATH" && ! -f "$BASELINE_PATH" ]]; then
  echo "Baseline file not found: $BASELINE_PATH" >&2
  exit 2
fi

python3 - "$REPO_ROOT" "$SRC_ROOT" "$MODE" "$OUTPUT_FORMAT" "$BASELINE_PATH" "$WRITE_PATH" "$CRITICAL_MODULES" <<'PY'
import datetime as dt
import fnmatch
import json
import re
import sys
from pathlib import Path

repo_root = Path(sys.argv[1]).resolve()
src_root = Path(sys.argv[2]).resolve()
mode = sys.argv[3]
output_format = sys.argv[4]
baseline_path_raw = sys.argv[5]
write_path_raw = sys.argv[6]
critical_modules_raw = sys.argv[7]

unwrap_re = re.compile(r"\.unwrap\(\)")
expect_re = re.compile(r"\.expect\(")
parser_expect_re = re.compile(r"\bself\s*\.\s*expect\(")
cfg_test_attr_re = re.compile(r"^\s*#\s*\[\s*cfg\s*\([^]]*test[^]]*\)\s*\]\s*$")

modules = [
    ("storage", Path("src/storage")),
    ("graph", Path("src/graph")),
    ("query", Path("src/query")),
    ("services", Path("src/services")),
    ("network_rest", Path("src/network/rest")),
    ("api_handlers", Path("src/api_handlers")),
    ("core", Path("src/core")),
]

def include_file(rel_path: Path) -> bool:
    if rel_path.suffix != ".rs":
        return False
    parts = rel_path.parts
    name = rel_path.name
    lower_name = name.lower()

    if "tests" in parts:
        return False
    if name.startswith("test_"):
        return False
    if name in {"test.rs", "tests.rs", "benchmark.rs", "benchmarks.rs", "example.rs", "examples.rs"}:
        return False
    if "benchmark" in lower_name or "example" in lower_name:
        return False
    if name.endswith("_test.rs") or name.endswith("_tests.rs"):
        return False
    if name == "comprehensive_test.rs":
        return False
    if fnmatch.fnmatch(name, "*legacy*.rs"):
        return False
    if any(part in {"benchmark", "benchmarks", "example", "examples"} for part in parts):
        return False
    return True


def strip_cfg_test_blocks(text: str) -> str:
    """Remove blocks gated behind #[cfg(test)] so counts represent production paths."""
    lines = text.splitlines(keepends=True)
    output = []
    i = 0

    while i < len(lines):
        line = lines[i]
        if cfg_test_attr_re.match(line):
            # Skip attribute lines chained with the cfg(test) attribute.
            j = i + 1
            while j < len(lines) and lines[j].lstrip().startswith("#["):
                j += 1

            # Skip the following item block (module/function/impl) or single-line item.
            k = j
            depth = 0
            started = False
            while k < len(lines):
                current = lines[k]
                for ch in current:
                    if ch == "{":
                        depth += 1
                        started = True
                    elif ch == "}" and started:
                        depth -= 1
                k += 1

                if started and depth <= 0:
                    break
                if not started and ";" in current:
                    break

            i = k
            continue

        output.append(line)
        i += 1

    return "".join(output)


def count_expect_calls(text: str) -> int:
    total = 0
    for line in text.splitlines():
        line_expect = len(expect_re.findall(line))
        if line_expect == 0:
            continue
        parser_expect = len(parser_expect_re.findall(line))
        total += max(0, line_expect - parser_expect)
    return total


def count_patterns() -> dict:
    totals = {"unwrap": 0, "expect": 0}
    module_counts = {
        name: {"path": str(path), "unwrap": 0, "expect": 0, "total": 0}
        for name, path in modules
    }

    for file_path in sorted(src_root.rglob("*.rs")):
        rel = file_path.relative_to(repo_root)
        if not include_file(rel):
            continue

        try:
            text = file_path.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue

        production_text = strip_cfg_test_blocks(text)

        unwrap_count = len(unwrap_re.findall(production_text))
        expect_count = count_expect_calls(production_text)

        if unwrap_count == 0 and expect_count == 0:
            continue

        totals["unwrap"] += unwrap_count
        totals["expect"] += expect_count

        for module_name, module_path in modules:
            if rel.is_relative_to(module_path):
                module_counts[module_name]["unwrap"] += unwrap_count
                module_counts[module_name]["expect"] += expect_count

    totals["total"] = totals["unwrap"] + totals["expect"]
    for module_name in module_counts:
        module_counts[module_name]["total"] = (
            module_counts[module_name]["unwrap"] + module_counts[module_name]["expect"]
        )

    return {"totals": totals, "modules": module_counts}


metrics = count_patterns()
payload = {
    "generated_at_utc": dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
    "repo": repo_root.name,
    "source_root": "src",
    "scan_scope": (
        "src/**/*.rs excluding tests/legacy/test-benchmark-example files and "
        "#[cfg(test)] blocks; ignores parser-internal self.expect(...) calls"
    ),
    "totals": metrics["totals"],
    "modules": metrics["modules"],
}

baseline = None
if baseline_path_raw:
    baseline_path = Path(baseline_path_raw).resolve()
    baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
    payload["baseline"] = {
        "path": str(baseline_path),
        "generated_at_utc": baseline.get("generated_at_utc"),
        "totals": baseline.get("totals", {}),
    }

violations = []
critical_modules = [x.strip() for x in critical_modules_raw.split(",") if x.strip()]

if baseline is not None:
    baseline_totals = baseline.get("totals", {})
    deltas = {
        "unwrap": payload["totals"]["unwrap"] - int(baseline_totals.get("unwrap", 0)),
        "expect": payload["totals"]["expect"] - int(baseline_totals.get("expect", 0)),
        "total": payload["totals"]["total"] - int(baseline_totals.get("total", 0)),
    }
    payload["delta_from_baseline"] = deltas

    if mode in {"no-regression", "module-guard"}:
        if deltas["total"] > 0:
            violations.append(
                f"total panic-prone calls increased by {deltas['total']} "
                f"({baseline_totals.get('total', 0)} -> {payload['totals']['total']})"
            )

    if mode == "module-guard":
        baseline_modules = baseline.get("modules", {})
        module_deltas = {}
        for module_name in critical_modules:
            current_module = payload["modules"].get(module_name)
            baseline_module = baseline_modules.get(module_name, {})
            baseline_total = int(baseline_module.get("total", 0))
            current_total = int((current_module or {}).get("total", 0))
            delta = current_total - baseline_total
            module_deltas[module_name] = {
                "baseline_total": baseline_total,
                "current_total": current_total,
                "delta_total": delta,
            }
            if delta > 0:
                violations.append(
                    f"critical module '{module_name}' total increased by {delta} "
                    f"({baseline_total} -> {current_total})"
                )
        payload["critical_modules"] = module_deltas

payload["mode"] = mode
payload["status"] = "fail" if violations else "pass"
payload["violations"] = violations

if write_path_raw:
    write_path = Path(write_path_raw).resolve()
    write_path.parent.mkdir(parents=True, exist_ok=True)
    write_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")

if output_format == "json":
    print(json.dumps(payload, indent=2, sort_keys=True))
else:
    print("=== ProximaDB Panic Policy Metrics ===")
    print(f"Generated (UTC): {payload['generated_at_utc']}")
    print(f"Mode: {mode}")
    print("")
    print("Totals:")
    print(f"  unwrap(): {payload['totals']['unwrap']}")
    print(f"  expect(): {payload['totals']['expect']}")
    print(f"  TOTAL:    {payload['totals']['total']}")
    print("")
    print("Module breakdown (unwrap/expect/total):")
    for module_name in sorted(payload["modules"]):
        stats = payload["modules"][module_name]
        print(
            f"  {module_name:13} "
            f"{stats['unwrap']:6} / {stats['expect']:6} / {stats['total']:6}"
        )

    if baseline is not None:
        print("")
        print("Delta from baseline (unwrap/expect/total):")
        deltas = payload.get("delta_from_baseline", {})
        print(
            f"  {deltas.get('unwrap', 0):+6} / "
            f"{deltas.get('expect', 0):+6} / "
            f"{deltas.get('total', 0):+6}"
        )
        if mode == "module-guard":
            print("")
            print("Critical module deltas:")
            for module_name in critical_modules:
                module_delta = payload.get("critical_modules", {}).get(module_name, {})
                print(
                    f"  {module_name:13} "
                    f"{module_delta.get('delta_total', 0):+6} "
                    f"({module_delta.get('baseline_total', 0)} -> {module_delta.get('current_total', 0)})"
                )

    print("")
    if violations:
        print("Policy violations:")
        for violation in violations:
            print(f"  - {violation}")
    else:
        if mode == "report":
            print("Report mode: no policy enforcement.")
        else:
            print("Policy check passed.")

if violations and mode != "report":
    sys.exit(1)
PY

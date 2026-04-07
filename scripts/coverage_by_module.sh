#!/bin/bash
# Generate module-level coverage summary from cargo llvm-cov
# Usage: ./scripts/coverage_by_module.sh

set -e

echo "================================================"
echo "ProximaDB Coverage Report by Module"
echo "Generated: $(date '+%Y-%m-%d %H:%M')"
echo "================================================"
echo ""

# Run cargo llvm-cov and capture per-file report
cargo llvm-cov --lib --text 2>/dev/null | \
  grep "^src/" | \
  awk -F'|' '{
    # Parse: filename | regions | missed | cover% | ...
    file=$1
    gsub(/^ +| +$/, "", file)
    # Extract module name (first directory under src/)
    split(file, parts, "/")
    if (parts[2] != "") {
      module = parts[2]
    } else {
      module = "root"
    }
    # Parse line coverage columns (columns vary by format)
    # Use the line coverage percentage from the report
    lines = $7+0    # Lines
    missed = $8+0   # Missed lines
    cover = $9+0    # Coverage %

    module_lines[module] += lines
    module_missed[module] += missed
    module_files[module]++
  }
  END {
    printf "%-25s %8s %8s %8s %8s\n", "Module", "Files", "Lines", "Covered", "Pct"
    printf "%-25s %8s %8s %8s %8s\n", "-------------------------", "--------", "--------", "--------", "--------"
    for (m in module_lines) {
      covered = module_lines[m] - module_missed[m]
      pct = (module_lines[m] > 0) ? (covered * 100.0 / module_lines[m]) : 0
      printf "%-25s %8d %8d %8d %7.1f%%\n", m, module_files[m], module_lines[m], covered, pct
    }
  }' | sort -t'%' -k1 -rn

echo ""
echo "================================================"

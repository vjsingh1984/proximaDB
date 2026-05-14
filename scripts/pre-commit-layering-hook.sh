#!/usr/bin/env bash
# Pre-commit hook for canonical workspace layering validation.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LAYERING_SCRIPT="$SCRIPT_DIR/check-layering.sh"

if [ ! -f "$LAYERING_SCRIPT" ]; then
    echo "Warning: layering validation script not found at $LAYERING_SCRIPT"
    echo "Skipping layering check."
    exit 0
fi

chmod +x "$LAYERING_SCRIPT"

echo "Running workspace layering validation..."
"$LAYERING_SCRIPT"
echo "Layering validation passed."

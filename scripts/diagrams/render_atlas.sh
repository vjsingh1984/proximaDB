# Render every Mermaid diagram under docs/architecture-diagrams to PNG+SVG (export mode) or
# emit Kroki-rendered .md sidecars (--kroki). Mermaid is the ONLY source format (no .puml;
# *.puml is gitignored). Invoke with: bash scripts/diagrams/render_atlas.sh [--kroki]
set -euo pipefail
ATLAS_DIR="docs/architecture-diagrams"
MODE="${1:-export}"
require_cmd() { command -v "$1" >/dev/null 2>&1; }
render_mermaid_local() {
  local src="$1" out="${src%.*}"
  if require_cmd mmdc; then
    mmdc -i "$src" -o "$out.png" -b transparent
    mmdc -i "$src" -o "$out.svg" -b transparent
  else
    echo "  mmdc not found; posting $src to Kroki" >&2
    local b64; b64=$(python3 -c "import sys,zlib,base64; print(base64.urlsafe_b64encode(zlib.compress(sys.stdin.buffer.read(),9)).decode())" < "$src")
    curl -s "https://kroki.io/mermaid/png/$b64" -o "$out.png"
  fi
}
kroki_link() {
  local src="$1" out="${src%.*}.kroki.md"
  local b64; b64=$(python3 -c "import sys,zlib,base64; print(base64.urlsafe_b64encode(zlib.compress(sys.stdin.buffer.read(),9)).decode())" < "$src")
  {
    echo "# $(basename "$src") (Kroki render)"
    echo
    echo "![$(basename "$src")](https://kroki.io/mermaid/svg/$b64)"
    echo
    echo "_Source: [\`$(basename "$src")\`]($(echo "$src" | sed "s|^$ATLAS_DIR/||"))_"
  } > "$out"
  echo "  wrote $out"
}
mapfile -t MERMAIDS < <(find "$ATLAS_DIR" -type f \( -name '*.mermaid' -o -name '*.mmd' \) | sort)
if [ "${#MERMAIDS[@]}" -eq 0 ]; then echo "no mermaid sources found under $ATLAS_DIR"; exit 0; fi
case "$MODE" in
  --kroki)
    echo "Kroki mode: emitting .kroki.md sidecars for ${#MERMAIDS[@]} diagrams"
    for f in "${MERMAIDS[@]}"; do kroki_link "$f"; done
    ;;
  export|"")
    echo "Export mode: PNG+SVG next to each source (${#MERMAIDS[@]} diagrams)"
    for f in "${MERMAIDS[@]}"; do echo "  mermaid: $f"; render_mermaid_local "$f"; done
    ;;
  *) echo "usage: $0 [--kroki]"; exit 2 ;;
esac
echo "done."

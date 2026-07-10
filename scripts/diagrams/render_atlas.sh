set -euo pipefail
ATLAS_DIR="docs/architecture-diagrams"
MODE="${1:-export}"
require_cmd() { command -v "$1" >/dev/null 2>&1; }
render_puml_local() {
  local src="$1" out="${1%.puml}"
  if require_cmd docker; then
    docker run --rm -i -v "$PWD:/data" -w /data \
      ghcr.io/plantuml/plantuml:latest -tpng -o "$(dirname "$src")" "$src"
  else
    echo "  docker not found; posting $src to Kroki" >&2
    local b64; b64=$(python3 -c "import sys,zlib,base64; print(base64.urlsafe_b64encode(zlib.compress(sys.stdin.buffer.read(),9)).decode())" < "$src")
    curl -s "https://kroki.io/plantuml/png/$b64" -o "$out.png"
  fi
}
render_mmd_local() {
  local src="$1" out="${1%.mmd}"
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
  local dtype="$1" src="$2" out="${2%.*}.kroki.md"
  local b64; b64=$(python3 -c "import sys,zlib,base64; print(base64.urlsafe_b64encode(zlib.compress(sys.stdin.buffer.read(),9)).decode())" < "$src")
  {
    echo "# $(basename "$src") (Kroki render)"
    echo
    echo "![$(basename "$src")](https://kroki.io/$dtype/svg/$b64)"
    echo
    echo "_Source: [\`$(basename "$src")\`]($(echo "$src" | sed "s|^$ATLAS_DIR/||"))_"
  } > "$out"
  echo "  wrote $out"
}
mapfile -t PUMLS < <(find "$ATLAS_DIR" -name '*.puml' -type f)
mapfile -t MMDS  < <(find "$ATLAS_DIR" -name '*.mmd' -type f)
case "$MODE" in
  --kroki)
    echo "Kroki mode: emitting .kroki.md sidecars"
    for f in "${PUMLS[@]}"; do kroki_link plantuml "$f"; done
    for f in "${MMDS[@]}";  do kroki_link mermaid  "$f"; done
    ;;
  export|"")
    echo "Export mode: PNG+SVG next to each source"
    for f in "${PUMLS[@]}"; do echo "  puml: $f"; render_puml_local "$f"; done
    for f in "${MMDS[@]}";  do echo "  mmd:  $f"; render_mmd_local "$f"; done
    ;;
  *) echo "usage: $0 [--kroki]"; exit 2 ;;
esac
echo "done."

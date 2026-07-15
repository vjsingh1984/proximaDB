#!/usr/bin/env bash
# TD-OBJSTORE-2: destructive, process-boundary durability proof over a unique
# prefix below PROXIMADB_OBJECT_STORE_URL. The Rust harness creates its own UUID
# sub-prefix and three fresh local VM directories; it never deletes user data.

set -euo pipefail

if [[ -z "${PROXIMADB_OBJECT_STORE_URL:-}" ]]; then
  echo "PROXIMADB_OBJECT_STORE_URL is required (s3://bucket/prefix or adls://container/prefix)" >&2
  exit 2
fi

case "${PROXIMADB_OBJECT_STORE_URL}" in
  s3://*) feature=aws ;;
  adls://*) feature=azure ;;
  *)
    echo "unsupported proof URL: ${PROXIMADB_OBJECT_STORE_URL} (expected s3:// or adls://)" >&2
    exit 2
    ;;
esac

echo "TD-OBJSTORE-2 durability proof"
echo "  object root: ${PROXIMADB_OBJECT_STORE_URL}"
echo "  backend feature: ${feature}"
echo "  engines: SST + HELIX"
echo "  phases: crash/WAL replay -> graceful flush -> fresh-disk cold read"

cargo test \
  -p proximadb-server \
  --features "${feature}" \
  --test object_store_restart_recovery \
  catalog_wal_and_sst_survive_fresh_local_disks \
  -- --ignored --nocapture --test-threads=1

echo "PASS: catalog recovered from metadata_url, WAL replay reattached SST + HELIX collections, and both engines served a second fresh-disk restart"

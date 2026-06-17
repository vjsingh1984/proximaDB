#!/usr/bin/env bash
set -euo pipefail

BENCHBASE_DIR="${BENCHBASE_DIR:-/Users/vijaysingh/code/benchbase}"
PROXIMADB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIST_DIR="${BENCHBASE_DIR}/target/benchbase-postgres"
RESULT_DIR="${PROXIMADB_DIR}/target/benchbase-results"

if [[ ! -f "${DIST_DIR}/benchbase.jar" ]]; then
  echo "BenchBase postgres distribution is missing."
  echo "Build it with: cd ${BENCHBASE_DIR} && ./mvnw clean package -P postgres && cd target && tar xzf benchbase-postgres.tgz"
  exit 1
fi

mkdir -p "${RESULT_DIR}"

cd "${DIST_DIR}"

java -jar benchbase.jar \
  -b tpcc \
  -c "${PROXIMADB_DIR}/tools/benchbase/proximadb_tpcc_100k.xml" \
  --create=true --load=true --execute=true \
  -d "${RESULT_DIR}/tpcc"

java -jar benchbase.jar \
  -b tpch \
  -c "${PROXIMADB_DIR}/tools/benchbase/proximadb_tpch_100k.xml" \
  --create=true --load=true --execute=true \
  -d "${RESULT_DIR}/tpch"

#!/usr/bin/env bash
# TD-097 pilot smoke test runner — builds the Rust cdylib, compiles the
# Java test harness, and runs the JNI round trip against the local
# `libproximadb_spark_jni.dylib` (or .so on Linux).
#
# Invoke from anywhere; the script cd's to its own directory then walks
# up to the repo root.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

echo "== 1/3  build Rust cdylib"
cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p proximadb-spark-jni

echo "== 2/3  compile Java"
BUILD="$SCRIPT_DIR/build"
mkdir -p "$BUILD"
javac -d "$BUILD" "$SCRIPT_DIR"/src/test/java/org/proximadb/spark/*.java

echo "== 3/3  run JNI smoke"
java -cp "$BUILD" \
     -Djava.library.path="$REPO_ROOT/target/debug" \
     org.proximadb.spark.NativeProximaDBTest

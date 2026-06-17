#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
SPARK_JAVA_DIR="$REPO_ROOT/clients/jvm/spark-connector/src/test/java/org/proximadb/spark"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
PROFILE_DIR="$TARGET_DIR/debug"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/proximadb-spark-jni-cross.XXXXXX")"
DATA_DIR="$WORK_DIR/shared-data"
BUILD_DIR="$WORK_DIR/classes"
SRC_DIR="$WORK_DIR/src/org/proximadb/spark"
FIRST_LOG="$WORK_DIR/first.log"
SECOND_LOG="$WORK_DIR/second.log"
THIRD_LOG="$WORK_DIR/third.log"

first_pid=""
second_pid=""

cleanup() {
  if [[ -n "${second_pid:-}" ]] && kill -0 "$second_pid" 2>/dev/null; then
    kill "$second_pid" 2>/dev/null || true
    wait "$second_pid" 2>/dev/null || true
  fi
  if [[ -n "${first_pid:-}" ]] && kill -0 "$first_pid" 2>/dev/null; then
    kill "$first_pid" 2>/dev/null || true
    wait "$first_pid" 2>/dev/null || true
  fi
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p proximadb-spark-jni
fi

if ! compgen -G "$PROFILE_DIR/*proximadb_spark_jni*" >/dev/null; then
  echo "missing Spark JNI library under $PROFILE_DIR"
  echo "run: cargo build --manifest-path $REPO_ROOT/Cargo.toml -p proximadb-spark-jni"
  exit 1
fi

mkdir -p "$BUILD_DIR" "$DATA_DIR" "$SRC_DIR"
cp "$SPARK_JAVA_DIR/NativeProximaDB.java" "$SRC_DIR/"

cat > "$SRC_DIR/InitializeHold.java" <<'JAVA'
package org.proximadb.spark;

public final class InitializeHold {
    private InitializeHold() {
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 2) {
            throw new IllegalArgumentException("usage: InitializeHold <dataDir> <holdMillis>");
        }
        boolean ok = NativeProximaDB.initialize(args[0]);
        System.out.println("INIT_OK=" + ok);
        System.out.flush();

        long holdMillis = Long.parseLong(args[1]);
        if (ok && holdMillis > 0) {
            Thread.sleep(holdMillis);
        }
        System.exit(ok ? 0 : 2);
    }
}
JAVA

javac -d "$BUILD_DIR" "$SRC_DIR/"*.java

java_cmd=(
  java
  -cp "$BUILD_DIR"
  -Djava.library.path="$PROFILE_DIR"
  org.proximadb.spark.InitializeHold
)

"${java_cmd[@]}" "$DATA_DIR" 5000 >"$FIRST_LOG" 2>&1 &
first_pid=$!

for _ in $(seq 1 100); do
  if grep -q "INIT_OK=true" "$FIRST_LOG"; then
    break
  fi
  if ! kill -0 "$first_pid" 2>/dev/null; then
    echo "first JVM exited before successful initialize"
    cat "$FIRST_LOG"
    exit 1
  fi
  sleep 0.1
done

if ! grep -q "INIT_OK=true" "$FIRST_LOG"; then
  echo "first JVM did not initialize before timeout"
  cat "$FIRST_LOG"
  exit 1
fi

"${java_cmd[@]}" "$DATA_DIR" 0 >"$SECOND_LOG" 2>&1 &
second_pid=$!
sleep 1

if grep -q "INIT_OK=true" "$SECOND_LOG"; then
  echo "second JVM initialized concurrently against the same exclusive data dir"
  cat "$SECOND_LOG"
  exit 1
fi

if kill -0 "$second_pid" 2>/dev/null; then
  echo "second JVM blocked while first JVM held the exclusive data dir"
  kill "$second_pid" 2>/dev/null || true
  wait "$second_pid" 2>/dev/null || true
  second_pid=""
elif grep -q "INIT_OK=false" "$SECOND_LOG"; then
  echo "second JVM rejected initialize while first JVM held the exclusive data dir"
  wait "$second_pid" 2>/dev/null || true
  second_pid=""
else
  echo "second JVM exited unexpectedly while first JVM held the exclusive data dir"
  cat "$SECOND_LOG"
  exit 1
fi

wait "$first_pid"
first_pid=""

"${java_cmd[@]}" "$DATA_DIR" 0 >"$THIRD_LOG" 2>&1

if ! grep -q "INIT_OK=true" "$THIRD_LOG"; then
  echo "third JVM did not initialize after first JVM released the data dir"
  cat "$THIRD_LOG"
  exit 1
fi

echo "PASS Spark JNI cross-process coordination smoke"

// Copyright 2025 ProximaDB
// Licensed under the Apache License, Version 2.0 (the "License").

package org.proximadb.spark;

/**
 * Standalone JUnit-style smoke test for the Spark connector JNI shim.
 *
 * Designed to run WITHOUT Gradle / Maven for the TD-097 pilot. The
 * companion script {@code clients/jvm/spark-connector/run-smoke.sh}
 * does:
 *
 * <pre>
 *   cargo build -p proximadb-spark-jni
 *   javac -d build src/test/java/org/proximadb/spark/*.java
 *   java -cp build -Djava.library.path=$REPO/target/debug \
 *       org.proximadb.spark.NativeProximaDBTest
 * </pre>
 *
 * No JUnit dependency: this is a plain {@code public static void main}
 * that exits non-zero on assertion failure. Gradle + proper JUnit
 * harness lands when the full TD-097 acceptance criteria are tackled.
 */
public final class NativeProximaDBTest {

    public static void main(String[] args) {
        int failures = 0;

        failures += run("getTableSchema returns scaffold JSON shape", () -> {
            String json = NativeProximaDB.getTableSchema("any_table");
            assertContains(json, "\"type\"", "schema JSON must carry top-level `type` field");
            assertContains(json, "\"struct\"", "scaffold returns type=struct per src/connectors/spark.rs");
            assertContains(json, "\"fields\"", "schema JSON must carry top-level `fields` field");
        });

        failures += run("getTableSchema handles empty table name", () -> {
            String json = NativeProximaDB.getTableSchema("");
            // The Rust scaffold ignores the input; what matters here is
            // that the JNI round trip doesn't crash on an empty string.
            if (json == null || json.isEmpty()) {
                throw new AssertionError("native call returned null/empty for empty input");
            }
        });

        failures += run("getTableSchema handles UTF-8 table name", () -> {
            String json = NativeProximaDB.getTableSchema("tαble_名前");
            assertContains(json, "\"type\"", "JNI must round-trip UTF-8 input without panicking");
        });

        if (failures == 0) {
            System.out.println("PASS  NativeProximaDBTest  3/3 smoke checks");
        } else {
            System.err.println("FAIL  NativeProximaDBTest  " + failures + " failure(s)");
            System.exit(1);
        }
    }

    private static int run(String name, ThrowingCheck check) {
        try {
            check.run();
            System.out.println("  ok   " + name);
            return 0;
        } catch (Throwable t) {
            System.err.println("  FAIL " + name + " — " + t.getMessage());
            return 1;
        }
    }

    private static void assertContains(String haystack, String needle, String msg) {
        if (haystack == null || !haystack.contains(needle)) {
            throw new AssertionError(msg + " (got: " + haystack + ")");
        }
    }

    @FunctionalInterface
    private interface ThrowingCheck {
        void run() throws Exception;
    }
}

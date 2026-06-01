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

        failures += run("planInputPartitions returns JSON array", () -> {
            String json = NativeProximaDB.planInputPartitions("t", "{}", 4);
            if (json == null || !json.trim().startsWith("[")) {
                throw new AssertionError("expected JSON array; got: " + json);
            }
        });

        failures += run("createPartitionReader returns handle (scaffold: 0)", () -> {
            long h = NativeProximaDB.createPartitionReader("{}");
            if (h != 0L) {
                throw new AssertionError("scaffold should return 0; got: " + h);
            }
        });

        failures += run("readNextBatch returns empty byte[] when exhausted", () -> {
            byte[] bytes = NativeProximaDB.readNextBatch(0L);
            if (bytes == null) {
                throw new AssertionError("readNextBatch must not return null");
            }
            if (bytes.length != 0) {
                throw new AssertionError("scaffold should return empty bytes; got len=" + bytes.length);
            }
        });

        failures += run("closePartitionReader is void / safe to call", () -> {
            NativeProximaDB.closePartitionReader(0L);
            NativeProximaDB.closePartitionReader(0L); // idempotent
        });

        failures += run("createDataWriter returns handle (scaffold: 0)", () -> {
            long h = NativeProximaDB.createDataWriter("t", "{}", 0);
            if (h != 0L) {
                throw new AssertionError("scaffold should return 0; got: " + h);
            }
        });

        failures += run("writeBatch is void / accepts byte[]", () -> {
            byte[] payload = new byte[]{0x41, 0x52, 0x52, 0x4f, 0x57}; // "ARROW"
            NativeProximaDB.writeBatch(0L, payload);
            NativeProximaDB.writeBatch(0L, new byte[0]); // empty payload tolerated
        });

        failures += run("commitWriter returns JSON commit metadata", () -> {
            String json = NativeProximaDB.commitWriter(0L);
            assertContains(json, "\"records_written\"",
                    "commit metadata must report records_written");
            assertContains(json, "\"bytes_written\"",
                    "commit metadata must report bytes_written");
        });

        failures += run("abortWriter is void / safe to call", () -> {
            NativeProximaDB.abortWriter(0L);
        });

        int total = 11; // 3 schema + 8 new
        if (failures == 0) {
            System.out.println("PASS  NativeProximaDBTest  " + total + "/" + total + " smoke checks");
        } else {
            System.err.println("FAIL  NativeProximaDBTest  " + failures + " of " + total + " failure(s)");
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

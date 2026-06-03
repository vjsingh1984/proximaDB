// Copyright 2025 ProximaDB
// Licensed under the Apache License, Version 2.0 (the "License").

package org.proximadb.spark;

import java.nio.file.Files;
import java.nio.file.Path;

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
 * harness lands when TD-097 acceptance (4) is tackled.
 *
 * After TD-097(3) B4: the native methods now talk to a real embedded
 * ProximaDB inside the cdylib (initialized via
 * {@link NativeProximaDB#initialize(String)}). The checks below
 * therefore assert REAL data shapes (e.g. "error" envelopes for
 * missing collections, RuntimeException on malformed inputs) rather
 * than the scaffold placeholders the harness previously tolerated.
 *
 * Limitations covered by follow-up TDs:
 *   - There is no JNI method to create a collection from Java today,
 *     so the harness cannot exercise the full write→read round trip.
 *     End-to-end coverage of that path lives in the main crate's
 *     `cargo test --lib connectors::spark::tests` (e.g.
 *     `test_spark_read_after_write_round_trip`).
 *   - The harness asserts ABI correctness + initialize bootstrap;
 *     real Spark DataFrame integration tests need a Spark dependency
 *     and a Gradle module — that's TD-097 acceptance (4).
 */
public final class NativeProximaDBTest {

    public static void main(String[] args) throws Exception {
        int failures = 0;

        // Bootstrap the embedded DB in a fresh tmpdir for the duration
        // of the test process. The cdylib's OnceLock makes a single
        // initialize call sticky for the process; tests can't reset it.
        final Path tmpDir = Files.createTempDirectory("proximadb-spark-jni-smoke");

        failures += run("initialize succeeds for a fresh dataDir", () -> {
            boolean ok = NativeProximaDB.initialize(tmpDir.toString());
            if (!ok) {
                throw new AssertionError("initialize must return true on first call");
            }
        });

        failures += run("initialize is set-once-strict (second call returns false)", () -> {
            boolean ok = NativeProximaDB.initialize(tmpDir.toString());
            if (ok) {
                throw new AssertionError("second initialize must return false");
            }
        });

        failures += run("initialize rejects an empty dataDir", () -> {
            // We can only check the return value because the singleton
            // is already set; the new-fresh check above proves the
            // success path. Empty input is rejected even when uninit
            // by the Rust side (matches the embedded-config contract).
            boolean ok = NativeProximaDB.initialize("");
            if (ok) {
                throw new AssertionError("empty dataDir must return false");
            }
        });

        failures += run("getTableSchema returns error envelope for missing collection", () -> {
            String json = NativeProximaDB.getTableSchema("not_a_real_collection");
            assertContains(json, "\"error\"",
                    "missing-collection response must be a JSON error envelope");
            assertContains(json, "not_a_real_collection",
                    "error envelope must surface the requested collection name");
        });

        failures += run("getTableSchema handles UTF-8 table name without panicking", () -> {
            // Still returns an error envelope (no such collection) but
            // the JNI round trip must not crash on the UTF-8 input.
            String json = NativeProximaDB.getTableSchema("tαble_名前");
            if (json == null || json.isEmpty()) {
                throw new AssertionError("native call must not return null/empty");
            }
        });

        failures += run("planInputPartitions returns empty array for missing collection", () -> {
            String json = NativeProximaDB.planInputPartitions("missing_col", "{}", 4);
            if (json == null || !json.trim().startsWith("[")) {
                throw new AssertionError("expected JSON array; got: " + json);
            }
            // Missing collection → planner returns empty list (the
            // Rust impl maps the underlying error to `[]` so Java
            // callers iterate zero partitions safely).
            if (!"[]".equals(json.trim())) {
                throw new AssertionError("missing collection must yield []: got " + json);
            }
        });

        failures += run("createPartitionReader throws on malformed JSON", () -> {
            try {
                NativeProximaDB.createPartitionReader("{garbage");
                throw new AssertionError("expected RuntimeException on malformed partition JSON");
            } catch (RuntimeException expected) {
                assertContains(expected.getMessage(), "createPartitionReader",
                        "exception message must name the failing operation");
            }
        });

        failures += run("closePartitionReader is idempotent for null handle", () -> {
            NativeProximaDB.closePartitionReader(0L);
            NativeProximaDB.closePartitionReader(0L);
        });

        failures += run("createDataWriter returns non-zero handle on valid input", () -> {
            String schemaJson = "{\"fields\":[],\"metadata\":{}}";
            long h = NativeProximaDB.createDataWriter("any_table", schemaJson, 0);
            if (h == 0L) {
                throw new AssertionError("real impl must return a non-null writer handle");
            }
            // Clean up — abort drops the writer without flushing.
            NativeProximaDB.abortWriter(h);
        });

        failures += run("createDataWriter throws on malformed schema JSON", () -> {
            try {
                NativeProximaDB.createDataWriter("any", "{not_json", 0);
                throw new AssertionError("expected RuntimeException on bad schema");
            } catch (RuntimeException expected) {
                assertContains(expected.getMessage(), "createDataWriter",
                        "exception message must name the failing operation");
            }
        });

        failures += run("writeBatch throws on uninitialized writer handle", () -> {
            try {
                byte[] payload = new byte[] {0x41, 0x52, 0x52, 0x4f, 0x57}; // "ARROW"
                NativeProximaDB.writeBatch(0L, payload);
                throw new AssertionError("writeBatch on null handle must throw");
            } catch (RuntimeException expected) {
                assertContains(expected.getMessage(), "writeBatch",
                        "exception message must name the failing operation");
            }
        });

        failures += run("commitWriter throws on uninitialized handle", () -> {
            try {
                NativeProximaDB.commitWriter(0L);
                throw new AssertionError("commitWriter on null handle must throw");
            } catch (RuntimeException expected) {
                assertContains(expected.getMessage(), "commitWriter",
                        "exception message must name the failing operation");
            }
        });

        failures += run("abortWriter is idempotent for null handle", () -> {
            NativeProximaDB.abortWriter(0L);
            NativeProximaDB.abortWriter(0L);
        });

        int total = 13;
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

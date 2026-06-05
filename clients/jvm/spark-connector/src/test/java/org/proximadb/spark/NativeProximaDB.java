// Copyright 2025 ProximaDB
// Licensed under the Apache License, Version 2.0 (the "License").

package org.proximadb.spark;

/**
 * JNI shim for the ProximaDB Spark connector helpers.
 *
 * The native implementations live in `crates/binding/proximadb-spark-jni`
 * and ship as `libproximadb_spark_jni.{dylib,so,dll}` after
 * `cargo build -p proximadb-spark-jni`. Each method here corresponds 1:1
 * to a `pub fn jni_*` in `src/connectors/spark.rs` of the main crate;
 * the cdylib wrapper exposes them with the JVM-mangled
 * `Java_org_proximadb_spark_NativeProximaDB_*` symbol names.
 *
 * TD-097 (docs/10-quality/TECHNICAL_DEBT.adoc) tracks post-MVP Spark
 * hardening: Gradle/JUnit harness migration, JVM DataSource V2 integration,
 * and shard-aware partition planning.
 */
public final class NativeProximaDB {

    /**
     * Load the native library once when the class is initialized.
     * Callers can override the library lookup path with the standard
     * `-Djava.library.path=...` JVM flag (the test harness sets it to
     * `target/debug/` so manual `javac` + `java` runs work without
     * installing the library system-wide).
     */
    static {
        System.loadLibrary("proximadb_spark_jni");
    }

    private NativeProximaDB() {
        // utility class; not instantiable
    }

    /**
     * Bootstrap the embedded ProximaDB instance the JNI shim wraps.
     * MUST be called exactly once per JVM process before any other
     * native method below. Returns {@code true} on success,
     * {@code false} if it was already initialized OR if the embedded
     * DB construction failed. Set-once-strict: a second
     * {@code initialize(differentDir)} returns false rather than
     * silently reconfiguring.
     */
    public static native boolean initialize(String dataDir);

    /** Fetch the table schema. Returns {@code {"error":"..."}} JSON envelope on failure. */
    public static native String getTableSchema(String tableName);

    /**
     * Plan input partitions for a Spark DataSource V2 scan.
     * Returns a JSON array of partition descriptors. The current embedded
     * implementation uses a correct single-partition fallback until
     * shard-aware Spark planning ships.
     */
    public static native String planInputPartitions(
            String tableName, String filtersJson, int numPartitions);

    /**
     * Open a partition reader and return an opaque native handle. Pass
     * the handle to {@link #readNextBatch(long)} and {@link
     * #closePartitionReader(long)}.
     */
    public static native long createPartitionReader(String partitionJson);

    /**
     * Read the next Arrow IPC RecordBatch from the partition reader.
     * Returns an empty byte array when the reader is exhausted.
     */
    public static native byte[] readNextBatch(long readerHandle);

    /** Release the partition reader. Safe to call multiple times. */
    public static native void closePartitionReader(long readerHandle);

    /**
     * Open a data writer for the given table + Spark partition.
     * Returns an opaque native handle. Pass to {@link #writeBatch(long, byte[])}
     * and {@link #commitWriter(long)} / {@link #abortWriter(long)}.
     */
    public static native long createDataWriter(
            String tableName, String schemaJson, int partitionId);

    /** Write one Arrow IPC RecordBatch to the data writer. */
    public static native void writeBatch(long writerHandle, byte[] arrowBatch);

    /**
     * Commit the data writer's pending writes and return commit metadata as JSON.
     */
    public static native String commitWriter(long writerHandle);

    /** Abort the data writer, rolling back any pending writes. */
    public static native void abortWriter(long writerHandle);
}

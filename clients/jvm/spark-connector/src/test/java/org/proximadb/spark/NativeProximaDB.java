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
 * TD-097 (docs/10-quality/TECHNICAL_DEBT.adoc) tracks the remaining
 * `jni_*` methods that still need a wrapper.
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

    /** Fetch the table schema. Scaffold returns {@code {"type":"struct","fields":[]}}. */
    public static native String getTableSchema(String tableName);

    /**
     * Plan input partitions for a Spark DataSource V2 scan.
     * Returns a JSON array of partition descriptors. Scaffold returns {@code []}.
     */
    public static native String planInputPartitions(
            String tableName, String filtersJson, int numPartitions);

    /**
     * Open a partition reader and return an opaque native handle. Pass
     * the handle to {@link #readNextBatch(long)} and {@link
     * #closePartitionReader(long)}. Scaffold returns 0.
     */
    public static native long createPartitionReader(String partitionJson);

    /**
     * Read the next Arrow IPC RecordBatch from the partition reader.
     * Returns an empty byte array when the reader is exhausted.
     * Scaffold always returns empty.
     */
    public static native byte[] readNextBatch(long readerHandle);

    /** Release the partition reader. Safe to call multiple times. */
    public static native void closePartitionReader(long readerHandle);

    /**
     * Open a data writer for the given table + Spark partition.
     * Returns an opaque native handle. Pass to {@link #writeBatch(long, byte[])}
     * and {@link #commitWriter(long)} / {@link #abortWriter(long)}. Scaffold returns 0.
     */
    public static native long createDataWriter(
            String tableName, String schemaJson, int partitionId);

    /** Write one Arrow IPC RecordBatch to the data writer. Scaffold is a no-op. */
    public static native void writeBatch(long writerHandle, byte[] arrowBatch);

    /**
     * Commit the data writer's pending writes and return commit metadata as JSON.
     * Scaffold returns {@code {"partition_id":0,"records_written":0,"bytes_written":0,"files_created":[]}}.
     */
    public static native String commitWriter(long writerHandle);

    /** Abort the data writer, rolling back any pending writes. Scaffold no-op. */
    public static native void abortWriter(long writerHandle);
}

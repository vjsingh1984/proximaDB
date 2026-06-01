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

    /**
     * Fetch the table schema for {@code tableName} via the Rust-side
     * `jni_get_table_schema`. Returns a JSON-encoded schema string
     * matching the Spark `StructType` JSON shape; today the Rust impl
     * is a scaffold that returns {@code {"type":"struct","fields":[]}}.
     *
     * Wiring to the underlying `ConnectorStorageAdapter.get_schema()`
     * is tracked under TD-097.
     */
    public static native String getTableSchema(String tableName);
}

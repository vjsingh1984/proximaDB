//! JNI shim exposing ProximaDB's Spark connector helpers to the JVM.
//!
//! The Spark DataSource V2 connector lives in `src/connectors/spark.rs` of
//! the main `proximadb` crate as pure-Rust scaffolds (the `jni_*` functions
//! return placeholder JSON / empty Vecs / 0). They are NOT callable from
//! Java as written — they lack the JNI calling convention, JNI-mangled
//! symbol names, and `JNIEnv` plumbing.
//!
//! This crate is the satellite cdylib that wraps each scaffold in a
//! proper `Java_<package>_<class>_<method>` export so the JVM can
//! `System.loadLibrary("proximadb_spark_jni")` and call them via JNA /
//! standard JNI.
//!
//! ## Why a separate crate
//!
//! The main `proximadb` crate is `crate-type = ["rlib"]` only (per
//! ADR-006: cdylib silently excludes inline test modules). Isolating
//! the cdylib output here keeps that constraint intact while still
//! producing a `libproximadb_spark_jni.{dylib,so,dll}` the JVM can load.
//!
//! ## Coverage — TD-097
//!
//! All 9 `jni_*` scaffolds in `src/connectors/spark.rs` are wrapped:
//!
//! | Rust scaffold | JNI export | Java signature |
//! |---|---|---|
//! | `jni_get_table_schema` | `…_getTableSchema` | `String getTableSchema(String)` |
//! | `jni_plan_input_partitions` | `…_planInputPartitions` | `String planInputPartitions(String, String, int)` |
//! | `jni_create_partition_reader` | `…_createPartitionReader` | `long createPartitionReader(String)` |
//! | `jni_read_next_batch` | `…_readNextBatch` | `byte[] readNextBatch(long)` |
//! | `jni_close_partition_reader` | `…_closePartitionReader` | `void closePartitionReader(long)` |
//! | `jni_create_data_writer` | `…_createDataWriter` | `long createDataWriter(String, String, int)` |
//! | `jni_write_batch` | `…_writeBatch` | `void writeBatch(long, byte[])` |
//! | `jni_commit_writer` | `…_commitWriter` | `String commitWriter(long)` |
//! | `jni_abort_writer` | `…_abortWriter` | `void abortWriter(long)` |
//!
//! The Rust-side scaffolds still return placeholder values; replacing
//! them with real `ConnectorStorageAdapter` calls is the remaining
//! TD-097 work. The wire-up below (JNI calling convention, mangled
//! symbol names, type-correct args/returns) is complete and proven by
//! `clients/jvm/spark-connector/src/test/java/org/proximadb/spark/NativeProximaDBTest.java`.

pub mod jni_handle;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jbyteArray, jint, jlong, jstring};

use proximadb::connectors::spark::{
    jni_abort_writer, jni_close_partition_reader, jni_commit_writer, jni_create_data_writer,
    jni_create_partition_reader, jni_get_table_schema, jni_plan_input_partitions,
    jni_read_next_batch, jni_write_batch,
};

/// Read a `JString` argument into an owned Rust `String`. Falls back to
/// an empty string on a JVM-level decode error (e.g. invalid UTF-16
/// surrogate pair); the underlying scaffolds tolerate empty input and
/// the caller can re-attempt with a corrected string.
fn jstring_to_string<'local>(env: &mut JNIEnv<'local>, s: JString<'local>) -> String {
    env.get_string(&s).map(Into::into).unwrap_or_default()
}

/// Convert an owned Rust `String` into a JNI-allocated `jstring`. Returns
/// the JNI null pointer on allocation failure — JNI convention for
/// "caller should check and treat as error".
fn string_to_jstring<'local>(env: &mut JNIEnv<'local>, s: String) -> jstring {
    env.new_string(s)
        .map(|js| js.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// JNI export wrapping [`jni_get_table_schema`].
///
/// Java signature:
/// ```java
/// package org.proximadb.spark;
/// public final class NativeProximaDB {
///     public static native String getTableSchema(String tableName);
/// }
/// ```
///
/// The JNI-mangled symbol name is
/// `Java_org_proximadb_spark_NativeProximaDB_getTableSchema`. The JVM
/// resolves this when `getTableSchema` is called after the cdylib is
/// loaded via `System.loadLibrary("proximadb_spark_jni")`.
///
/// # Safety
///
/// Standard JNI export contract: `env` and `class` are valid for the
/// call's duration; `table_name` is a JVM-owned `jstring` reference.
/// Implementation only reads `table_name` once via the safe
/// `get_string` helper, then constructs an owned Rust `String`. The
/// returned `jstring` is freshly allocated by `env.new_string` and
/// ownership passes back to the JVM.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_getTableSchema<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    table_name: JString<'local>,
) -> jstring {
    let table_name = jstring_to_string(&mut env, table_name);
    let schema_json = jni_get_table_schema(&table_name);
    string_to_jstring(&mut env, schema_json)
}

/// JNI export wrapping [`jni_plan_input_partitions`].
///
/// Java: `static native String planInputPartitions(String tableName,
///                                                  String filtersJson,
///                                                  int numPartitions);`
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_planInputPartitions<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    table_name: JString<'local>,
    filters_json: JString<'local>,
    num_partitions: jint,
) -> jstring {
    let table = jstring_to_string(&mut env, table_name);
    let filters = jstring_to_string(&mut env, filters_json);
    let partitions_json = jni_plan_input_partitions(&table, &filters, num_partitions);
    string_to_jstring(&mut env, partitions_json)
}

/// JNI export wrapping [`jni_create_partition_reader`].
///
/// Java: `static native long createPartitionReader(String partitionJson);`
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_createPartitionReader<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    partition_json: JString<'local>,
) -> jlong {
    let partition = jstring_to_string(&mut env, partition_json);
    jni_create_partition_reader(&partition)
}

/// JNI export wrapping [`jni_read_next_batch`].
///
/// Java: `static native byte[] readNextBatch(long readerHandle);`
///
/// Returns Arrow IPC serialized bytes, or an empty array when the
/// reader is exhausted. Allocation failure surfaces as the JNI null
/// pointer (Java sees a null byte[]).
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_readNextBatch<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    reader_handle: jlong,
) -> jbyteArray {
    let bytes = jni_read_next_batch(reader_handle);
    match env.byte_array_from_slice(&bytes) {
        Ok(arr) => arr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// JNI export wrapping [`jni_close_partition_reader`].
///
/// Java: `static native void closePartitionReader(long readerHandle);`
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_closePartitionReader<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    reader_handle: jlong,
) {
    jni_close_partition_reader(reader_handle);
}

/// JNI export wrapping [`jni_create_data_writer`].
///
/// Java: `static native long createDataWriter(String tableName,
///                                              String schemaJson,
///                                              int partitionId);`
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_createDataWriter<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    table_name: JString<'local>,
    schema_json: JString<'local>,
    partition_id: jint,
) -> jlong {
    let table = jstring_to_string(&mut env, table_name);
    let schema = jstring_to_string(&mut env, schema_json);
    jni_create_data_writer(&table, &schema, partition_id)
}

/// JNI export wrapping [`jni_write_batch`].
///
/// Java: `static native void writeBatch(long writerHandle, byte[] arrowBatch);`
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_writeBatch<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    writer_handle: jlong,
    arrow_batch: JByteArray<'local>,
) {
    let bytes = env.convert_byte_array(&arrow_batch).unwrap_or_default();
    jni_write_batch(writer_handle, &bytes);
}

/// JNI export wrapping [`jni_commit_writer`].
///
/// Java: `static native String commitWriter(long writerHandle);`
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_commitWriter<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    writer_handle: jlong,
) -> jstring {
    let commit_json = jni_commit_writer(writer_handle);
    string_to_jstring(&mut env, commit_json)
}

/// JNI export wrapping [`jni_abort_writer`].
///
/// Java: `static native void abortWriter(long writerHandle);`
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_abortWriter<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    writer_handle: jlong,
) {
    jni_abort_writer(writer_handle);
}

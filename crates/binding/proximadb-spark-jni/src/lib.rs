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

use std::sync::Arc;
use std::sync::OnceLock;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jbyteArray, jint, jlong, jstring};

use proximadb::connectors::spark::{
    SparkDataWriter, SparkPartitionReader, spark_abort_writer, spark_close_partition_reader,
    spark_commit_writer, spark_create_data_writer, spark_create_partition_reader,
    spark_get_table_schema, spark_plan_input_partitions, spark_read_next_batch, spark_write_batch,
};
use proximadb::embedded::{EmbeddedConfig, EmbeddedProximaDB};

/// Process-singleton embedded database. Set once by
/// `Java_..._initialize`; read by every other JNI wrapper. Tests
/// construct their own `EmbeddedProximaDB` directly and call the
/// pure-Rust `spark_*` impls — they do NOT touch this singleton.
///
/// Holds `Arc<EmbeddedProximaDB>` rather than `EmbeddedProximaDB`
/// directly so the JNI wrappers can clone the Arc cheaply per call
/// without contending on the OnceLock.
static EMBEDDED: OnceLock<Arc<EmbeddedProximaDB>> = OnceLock::new();

/// Get a clone of the embedded singleton's Arc handle. Returns `None`
/// if `initialize` has never been called.
fn embedded() -> Option<Arc<EmbeddedProximaDB>> {
    EMBEDDED.get().cloned()
}

/// JNI export: bootstrap the embedded database. Java callers MUST call
/// `initialize(dataDir)` exactly once before any other native method;
/// returns `true` on success, `false` if it's already been called
/// (set-once-strict: a second call with a DIFFERENT data_dir is a hard
/// programming error, but we return false instead of panicking
/// because JNI panics tear down the JVM).
///
/// Java: `static native boolean initialize(String dataDir);`
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_initialize<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    data_dir: JString<'local>,
) -> jboolean {
    let dir = jstring_to_string(&mut env, data_dir);
    if dir.is_empty() {
        return JNI_FALSE;
    }
    if EMBEDDED.get().is_some() {
        // Already initialized — second call is a no-op and returns
        // false so callers can detect dup-init in tests.
        return JNI_FALSE;
    }
    let mut config = EmbeddedConfig::for_low_memory(&dir);
    config.enable_wal = true;
    let db = match EmbeddedProximaDB::new(config) {
        Ok(db) => db,
        Err(_) => return JNI_FALSE,
    };
    match EMBEDDED.set(Arc::new(db)) {
        Ok(()) => JNI_TRUE,
        Err(_) => JNI_FALSE,
    }
}

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

/// Throw a `java.lang.RuntimeException` and return — used by JNI
/// wrappers that hit a `SparkError` they can't silently swallow
/// (writes, commits, malformed handles). Never panics; logs internally
/// on JNI failure since there's nothing safe to do at that point.
fn throw_runtime_exception<'local>(env: &mut JNIEnv<'local>, msg: String) {
    if env.throw_new("java/lang/RuntimeException", &msg).is_err() {
        eprintln!("proximadb-spark-jni: failed to throw java.lang.RuntimeException: {msg}");
    }
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
    let schema_json = match embedded() {
        Some(db) => spark_get_table_schema(&db, &table_name),
        None => r#"{"error":"not initialized — call NativeProximaDB.initialize(dataDir) first"}"#
            .to_string(),
    };
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
    let partitions_json = match embedded() {
        Some(db) => spark_plan_input_partitions(&db, &table, &filters, num_partitions),
        None => "[]".to_string(),
    };
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
    match spark_create_partition_reader(&partition) {
        Ok(reader) => jni_handle::leak::<SparkPartitionReader>(reader),
        Err(e) => {
            throw_runtime_exception(&mut env, format!("createPartitionReader: {e}"));
            0
        }
    }
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
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    reader_handle: jlong,
) -> jbyteArray {
    let Some(db) = embedded() else {
        throw_runtime_exception(
            &mut env,
            "readNextBatch: not initialized — call initialize(dataDir) first".to_string(),
        );
        return std::ptr::null_mut();
    };
    // SAFETY: caller (Java) guarantees the handle was produced by
    // createPartitionReader and not yet closed; the JNI ABI is single-
    // threaded per Spark task so no concurrent borrow / take.
    let reader = match unsafe { jni_handle::borrow_mut::<SparkPartitionReader>(reader_handle) } {
        Some(r) => r,
        None => {
            throw_runtime_exception(&mut env, "readNextBatch: null reader handle".to_string());
            return std::ptr::null_mut();
        }
    };
    let bytes = match spark_read_next_batch(&db, reader) {
        Ok(b) => b,
        Err(e) => {
            throw_runtime_exception(&mut env, format!("readNextBatch: {e}"));
            return std::ptr::null_mut();
        }
    };
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
    // SAFETY: Java side guarantees one close call per handle (matches
    // Spark `PartitionReader.close()` ABI). Null handles are silently
    // ignored — double-close is idempotent.
    if let Some(reader) = unsafe { jni_handle::take::<SparkPartitionReader>(reader_handle) } {
        spark_close_partition_reader(*reader);
    }
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
    match spark_create_data_writer(&table, &schema, partition_id) {
        Ok(writer) => jni_handle::leak::<SparkDataWriter>(writer),
        Err(e) => {
            throw_runtime_exception(&mut env, format!("createDataWriter: {e}"));
            0
        }
    }
}

/// JNI export wrapping [`jni_write_batch`].
///
/// Java: `static native void writeBatch(long writerHandle, byte[] arrowBatch);`
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_proximadb_spark_NativeProximaDB_writeBatch<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    writer_handle: jlong,
    arrow_batch: JByteArray<'local>,
) {
    let Some(db) = embedded() else {
        throw_runtime_exception(
            &mut env,
            "writeBatch: not initialized — call initialize(dataDir) first".to_string(),
        );
        return;
    };
    let bytes = env.convert_byte_array(&arrow_batch).unwrap_or_default();
    // SAFETY: Java guarantees the handle was produced by
    // createDataWriter and is not yet committed/aborted; single-task
    // ABI means no concurrent borrow.
    let writer = match unsafe { jni_handle::borrow_mut::<SparkDataWriter>(writer_handle) } {
        Some(w) => w,
        None => {
            throw_runtime_exception(&mut env, "writeBatch: null writer handle".to_string());
            return;
        }
    };
    if let Err(e) = spark_write_batch(&db, writer, &bytes) {
        throw_runtime_exception(&mut env, format!("writeBatch: {e}"));
    }
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
    let Some(db) = embedded() else {
        throw_runtime_exception(
            &mut env,
            "commitWriter: not initialized — call initialize(dataDir) first".to_string(),
        );
        return std::ptr::null_mut();
    };
    // SAFETY: caller-asserted (one commit call per handle, no live
    // borrows).
    let writer = match unsafe { jni_handle::take::<SparkDataWriter>(writer_handle) } {
        Some(w) => *w,
        None => {
            throw_runtime_exception(&mut env, "commitWriter: null writer handle".to_string());
            return std::ptr::null_mut();
        }
    };
    let commit_json = match spark_commit_writer(&db, writer) {
        Ok(j) => j,
        Err(e) => {
            throw_runtime_exception(&mut env, format!("commitWriter: {e}"));
            return std::ptr::null_mut();
        }
    };
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
    // SAFETY: caller-asserted; double-abort silently ignored (null
    // handle short-circuits in `take`).
    if let Some(writer) = unsafe { jni_handle::take::<SparkDataWriter>(writer_handle) } {
        spark_abort_writer(*writer);
    }
}

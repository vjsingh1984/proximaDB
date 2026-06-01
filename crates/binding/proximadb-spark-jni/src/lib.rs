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
//! ## Pilot scope — TD-097
//!
//! This commit ships ONE wrapped JNI function as the end-to-end proof:
//! `Java_org_proximadb_spark_NativeProximaDB_getTableSchema`. The other
//! 8 `jni_*` scaffolds in `src/connectors/spark.rs` mirror the same
//! pattern; wrapping them is mechanical follow-up work tracked under
//! the same TD.
//!
//! Java side: `clients/jvm/spark-connector/src/test/java/org/proximadb/spark/NativeProximaDBTest.java`.

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::jstring;

use proximadb::connectors::spark::jni_get_table_schema;

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
    let table_name: String = match env.get_string(&table_name) {
        Ok(s) => s.into(),
        Err(_) => String::new(),
    };

    let schema_json = jni_get_table_schema(&table_name);

    match env.new_string(schema_json) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

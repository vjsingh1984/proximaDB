//! Java JNI Bindings for ProximaDB Embedded Mode
//!
//! This module provides JNI (Java Native Interface) bindings for using
//! ProximaDB as an embedded database in Java applications.

use jni::JNIEnv;
use jni::objects::{JClass, JObject, JObjectArray, JString, JValue};
use jni::sys::{jfloatArray, jint, jlong, jobject, jobjectArray, jsize, jstring};
use std::collections::HashMap;
use std::sync::Arc;

use super::{EmbeddedConfig, EmbeddedProximaDB, StorageLocationConfig};

/// JNI wrapper holding the embedded database instance
struct JniProximaDB {
    db: Arc<EmbeddedProximaDB>,
}

/// Convert Java String to Rust String
fn jstring_to_string(env: &mut JNIEnv, jstr: &JString) -> Result<String, String> {
    env.get_string(jstr)
        .map(|s| s.into())
        .map_err(|e| format!("Failed to convert string: {}", e))
}

/// Convert Rust String to Java String
fn string_to_jstring<'a>(env: &mut JNIEnv<'a>, s: &str) -> Result<JString<'a>, String> {
    env.new_string(s)
        .map_err(|e| format!("Failed to create Java string: {}", e))
}

/// Throw a Java RuntimeException
fn throw_exception(env: &mut JNIEnv, message: &str) {
    let _ = env.throw_new("java/lang/RuntimeException", message);
}

/// Store the database pointer as a long in the Java object
fn set_db_pointer(env: &mut JNIEnv, obj: &JObject, db: Box<JniProximaDB>) -> Result<(), String> {
    let ptr = Box::into_raw(db) as jlong;
    env.set_field(obj, "nativeHandle", "J", JValue::Long(ptr))
        .map_err(|e| format!("Failed to set native handle: {}", e))
}

/// Get the database pointer from the Java object
fn get_db<'a>(env: &mut JNIEnv, obj: &JObject) -> Result<&'a JniProximaDB, String> {
    let ptr = env
        .get_field(obj, "nativeHandle", "J")
        .and_then(|v| v.j())
        .map_err(|e| format!("Failed to get native handle: {}", e))?;

    if ptr == 0 {
        return Err("Database not initialized".to_string());
    }

    Ok(unsafe { &*(ptr as *const JniProximaDB) })
}

// ============================================================================
// JNI Native Methods
// ============================================================================

/// Create a new ProximaDB instance
///
/// Java signature: native void nativeCreate(String dataDir, String metadataDir, int cacheSizeMb, String engine)
#[no_mangle]
pub extern "system" fn Java_com_proximadb_embedded_ProximaDB_nativeCreate(
    mut env: JNIEnv,
    obj: JObject,
    data_dir: JString,
    metadata_dir: JString,
    cache_size_mb: jint,
    engine: JString,
) {
    let result = (|| -> Result<(), String> {
        let data_path = jstring_to_string(&mut env, &data_dir)?;
        let metadata_path = if metadata_dir.is_null() {
            format!("{}/metadata", data_path)
        } else {
            jstring_to_string(&mut env, &metadata_dir)?
        };
        let engine_str = if engine.is_null() {
            "sst".to_string()
        } else {
            jstring_to_string(&mut env, &engine)?
        };

        let config = EmbeddedConfig {
            storage_locations: vec![StorageLocationConfig::new(data_path)],
            metadata_path,
            cache_size_mb: cache_size_mb as usize,
            default_engine: engine_str,
            enable_wal: true,
            wal_sync_mode: "batch".to_string(),
        };

        let db = EmbeddedProximaDB::new(config)
            .map_err(|e| format!("Failed to create database: {}", e))?;

        let jni_db = Box::new(JniProximaDB { db: Arc::new(db) });
        set_db_pointer(&mut env, &obj, jni_db)?;

        Ok(())
    })();

    if let Err(e) = result {
        throw_exception(&mut env, &e);
    }
}

/// Create a new ProximaDB instance with multi-disk support
#[no_mangle]
pub extern "system" fn Java_com_proximadb_embedded_ProximaDB_nativeCreateMultiDisk(
    mut env: JNIEnv,
    obj: JObject,
    disk_paths: JObjectArray,
    disk_weights: jfloatArray,
    metadata_dir: JString,
    cache_size_mb: jint,
    engine: JString,
) {
    let result = (|| -> Result<(), String> {
        // Get disk paths array
        let path_count = env
            .get_array_length(&disk_paths)
            .map_err(|e| format!("Failed to get array length: {}", e))?;

        let mut storage_locations = Vec::new();

        for i in 0..path_count {
            let path_obj = env
                .get_object_array_element(&disk_paths, i)
                .map_err(|e| format!("Failed to get path element: {}", e))?;
            let path_str: JString = path_obj.into();
            let path = jstring_to_string(&mut env, &path_str)?;

            // Get weight if available
            let weight = if !disk_weights.is_null() {
                let mut weights = vec![0.0f32; 1];
                unsafe {
                    env.get_float_array_region(
                        &jni::objects::JFloatArray::from_raw(disk_weights),
                        i,
                        &mut weights,
                    )
                    .map_err(|e| format!("Failed to get weight: {}", e))?;
                }
                weights[0] as u32
            } else {
                1
            };

            storage_locations.push(StorageLocationConfig::new(path).with_weight(weight));
        }

        let metadata_path = if metadata_dir.is_null() {
            format!("{}/metadata", storage_locations[0].path)
        } else {
            jstring_to_string(&mut env, &metadata_dir)?
        };

        let engine_str = if engine.is_null() {
            "sst".to_string()
        } else {
            jstring_to_string(&mut env, &engine)?
        };

        let config = EmbeddedConfig {
            storage_locations,
            metadata_path,
            cache_size_mb: cache_size_mb as usize,
            default_engine: engine_str,
            enable_wal: true,
            wal_sync_mode: "batch".to_string(),
        };

        let db = EmbeddedProximaDB::new(config)
            .map_err(|e| format!("Failed to create database: {}", e))?;

        let jni_db = Box::new(JniProximaDB { db: Arc::new(db) });
        set_db_pointer(&mut env, &obj, jni_db)?;

        Ok(())
    })();

    if let Err(e) = result {
        throw_exception(&mut env, &e);
    }
}

/// Close and cleanup the database
#[no_mangle]
pub extern "system" fn Java_com_proximadb_embedded_ProximaDB_nativeClose(
    mut env: JNIEnv,
    obj: JObject,
) {
    let result = (|| -> Result<(), String> {
        let ptr = env
            .get_field(&obj, "nativeHandle", "J")
            .and_then(|v| v.j())
            .map_err(|e| format!("Failed to get native handle: {}", e))?;

        if ptr != 0 {
            // Drop the database instance
            unsafe {
                let _ = Box::from_raw(ptr as *mut JniProximaDB);
            }

            // Set handle to null
            env.set_field(&obj, "nativeHandle", "J", JValue::Long(0))
                .map_err(|e| format!("Failed to clear handle: {}", e))?;
        }

        Ok(())
    })();

    if let Err(e) = result {
        throw_exception(&mut env, &e);
    }
}

/// Create a new collection
#[no_mangle]
pub extern "system" fn Java_com_proximadb_embedded_ProximaDB_nativeCreateCollection(
    mut env: JNIEnv,
    obj: JObject,
    name: JString,
    dimension: jint,
    engine: JString,
) {
    let result = (|| -> Result<(), String> {
        let db = get_db(&mut env, &obj)?;
        let name_str = jstring_to_string(&mut env, &name)?;
        let engine_opt = if engine.is_null() {
            None
        } else {
            Some(jstring_to_string(&mut env, &engine)?)
        };

        db.db
            .create_collection(&name_str, dimension as u32, engine_opt.as_deref())
            .map_err(|e| format!("Failed to create collection: {}", e))
    })();

    if let Err(e) = result {
        throw_exception(&mut env, &e);
    }
}

/// Delete a collection
#[no_mangle]
pub extern "system" fn Java_com_proximadb_embedded_ProximaDB_nativeDeleteCollection(
    mut env: JNIEnv,
    obj: JObject,
    name: JString,
) {
    let result = (|| -> Result<(), String> {
        let db = get_db(&mut env, &obj)?;
        let name_str = jstring_to_string(&mut env, &name)?;

        db.db
            .delete_collection(&name_str)
            .map_err(|e| format!("Failed to delete collection: {}", e))
    })();

    if let Err(e) = result {
        throw_exception(&mut env, &e);
    }
}

/// Insert vectors into a collection
#[no_mangle]
pub extern "system" fn Java_com_proximadb_embedded_ProximaDB_nativeInsert(
    mut env: JNIEnv,
    obj: JObject,
    collection: JString,
    ids: JObjectArray,
    vectors: JObjectArray, // Array of float[]
) -> jint {
    let result = (|| -> Result<jint, String> {
        let db = get_db(&mut env, &obj)?;
        let collection_name = jstring_to_string(&mut env, &collection)?;

        // Get IDs
        let id_count =
            env.get_array_length(&ids)
                .map_err(|e| format!("Failed to get IDs length: {}", e))? as usize;

        let mut rust_ids = Vec::with_capacity(id_count);
        for i in 0..id_count as jsize {
            let id_obj = env
                .get_object_array_element(&ids, i)
                .map_err(|e| format!("Failed to get ID element: {}", e))?;
            let id_str: JString = id_obj.into();
            rust_ids.push(jstring_to_string(&mut env, &id_str)?);
        }

        // Get vectors
        let vec_count =
            env.get_array_length(&vectors)
                .map_err(|e| format!("Failed to get vectors length: {}", e))? as usize;

        if vec_count != id_count {
            return Err("IDs and vectors must have same length".to_string());
        }

        let mut rust_vectors = Vec::with_capacity(vec_count);
        for i in 0..vec_count as jsize {
            let vec_obj = env
                .get_object_array_element(&vectors, i)
                .map_err(|e| format!("Failed to get vector element: {}", e))?;

            let float_array = jni::objects::JFloatArray::from(vec_obj);
            let vec_len = env
                .get_array_length(&float_array)
                .map_err(|e| format!("Failed to get vector length: {}", e))?
                as usize;

            let mut vec_data = vec![0.0f32; vec_len];
            env.get_float_array_region(&float_array, 0, &mut vec_data)
                .map_err(|e| format!("Failed to copy vector data: {}", e))?;

            rust_vectors.push(vec_data);
        }

        let count = db
            .db
            .insert(&collection_name, rust_ids, rust_vectors, None)
            .map_err(|e| format!("Insert failed: {}", e))?;

        Ok(count as jint)
    })();

    match result {
        Ok(count) => count,
        Err(e) => {
            throw_exception(&mut env, &e);
            -1
        }
    }
}

/// Search for similar vectors
#[no_mangle]
pub extern "system" fn Java_com_proximadb_embedded_ProximaDB_nativeSearch(
    mut env: JNIEnv,
    obj: JObject,
    collection: JString,
    query: jfloatArray,
    top_k: jint,
) -> jobjectArray {
    let result = (|| -> Result<jobjectArray, String> {
        let db = get_db(&mut env, &obj)?;
        let collection_name = jstring_to_string(&mut env, &collection)?;

        // Get query vector
        let query_array = unsafe { jni::objects::JFloatArray::from_raw(query) };
        let query_len =
            env.get_array_length(&query_array)
                .map_err(|e| format!("Failed to get query length: {}", e))? as usize;

        let mut query_vec = vec![0.0f32; query_len];
        env.get_float_array_region(&query_array, 0, &mut query_vec)
            .map_err(|e| format!("Failed to copy query data: {}", e))?;

        // Perform search
        let results = db
            .db
            .search(&collection_name, query_vec, top_k as usize, None)
            .map_err(|e| format!("Search failed: {}", e))?;

        // Create SearchResult array
        let search_result_class = env
            .find_class("com/proximadb/embedded/SearchResult")
            .map_err(|e| format!("Failed to find SearchResult class: {}", e))?;

        let result_array = env
            .new_object_array(
                results.len() as jsize,
                &search_result_class,
                JObject::null(),
            )
            .map_err(|e| format!("Failed to create result array: {}", e))?;

        for (i, result) in results.iter().enumerate() {
            let id = string_to_jstring(&mut env, &result.id)?;

            let result_obj = env
                .new_object(
                    &search_result_class,
                    "(Ljava/lang/String;F)V",
                    &[JValue::Object(&id.into()), JValue::Float(result.score)],
                )
                .map_err(|e| format!("Failed to create SearchResult: {}", e))?;

            env.set_object_array_element(&result_array, i as jsize, result_obj)
                .map_err(|e| format!("Failed to set result element: {}", e))?;
        }

        Ok(result_array.into_raw())
    })();

    match result {
        Ok(array) => array,
        Err(e) => {
            throw_exception(&mut env, &e);
            std::ptr::null_mut()
        }
    }
}

/// Flush pending writes
#[no_mangle]
pub extern "system" fn Java_com_proximadb_embedded_ProximaDB_nativeFlush(
    mut env: JNIEnv,
    obj: JObject,
) {
    let result = (|| -> Result<(), String> {
        let db = get_db(&mut env, &obj)?;
        db.db.flush().map_err(|e| format!("Flush failed: {}", e))
    })();

    if let Err(e) = result {
        throw_exception(&mut env, &e);
    }
}

//! C FFI Bindings for ProximaDB Embedded Mode
//!
//! This module provides C-compatible FFI bindings for using ProximaDB
//! from C, C++, Go (CGO), and other languages with C interop capabilities.

use std::ffi::{CStr, CString, c_char, c_float, c_int};
use std::ptr;
use std::sync::Arc;

use super::{EmbeddedConfig, EmbeddedProximaDB, StorageLocationConfig};

/// Opaque handle to a ProximaDB instance
pub struct ProximaDBHandle {
    db: Arc<EmbeddedProximaDB>,
}

/// Search result for C FFI
#[repr(C)]
pub struct CSearchResult {
    /// Vector ID (caller must free with proximadb_free_string)
    pub id: *mut c_char,
    /// Similarity score
    pub score: c_float,
}

/// Search results array for C FFI
#[repr(C)]
pub struct CSearchResults {
    /// Array of results
    pub results: *mut CSearchResult,
    /// Number of results
    pub count: c_int,
}

/// Error result for C FFI
#[repr(C)]
pub struct CResult {
    /// Success flag (0 = success, non-zero = error)
    pub error_code: c_int,
    /// Error message (null if success, caller must free with proximadb_free_string)
    pub error_message: *mut c_char,
}

impl Default for CResult {
    fn default() -> Self {
        Self {
            error_code: 0,
            error_message: ptr::null_mut(),
        }
    }
}

fn set_error(result: &mut CResult, code: c_int, message: &str) {
    result.error_code = code;
    if let Ok(cstr) = CString::new(message) {
        result.error_message = cstr.into_raw();
    }
}

fn success_result() -> CResult {
    CResult::default()
}

fn error_result(code: c_int, message: &str) -> CResult {
    let mut result = CResult::default();
    set_error(&mut result, code, message);
    result
}

// ============================================================================
// C FFI Functions
// ============================================================================

/// Create a new ProximaDB instance with single directory
///
/// # Safety
/// - data_dir must be a valid null-terminated string
/// - metadata_dir can be null
/// - engine can be null (will use default)
/// - Returns null on error
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_open(
    data_dir: *const c_char,
    metadata_dir: *const c_char,
    cache_size_mb: c_int,
    engine: *const c_char,
) -> *mut ProximaDBHandle {
    let data_path = match CStr::from_ptr(data_dir).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return ptr::null_mut(),
    };

    let metadata_path = if metadata_dir.is_null() {
        format!("{}/metadata", data_path)
    } else {
        match CStr::from_ptr(metadata_dir).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return ptr::null_mut(),
        }
    };

    let engine_str = if engine.is_null() {
        "sst".to_string()
    } else {
        match CStr::from_ptr(engine).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => "sst".to_string(),
        }
    };

    let config = EmbeddedConfig {
        storage_locations: vec![StorageLocationConfig::new(data_path)],
        metadata_path,
        cache_size_mb: cache_size_mb as usize,
        default_engine: engine_str,
        enable_wal: true,
        wal_sync_mode: "batch".to_string(),
        ..EmbeddedConfig::default()
    };

    match EmbeddedProximaDB::new(config) {
        Ok(db) => Box::into_raw(Box::new(ProximaDBHandle { db: Arc::new(db) })),
        Err(_) => ptr::null_mut(),
    }
}

/// Create a new ProximaDB instance with multi-disk support
///
/// # Safety
/// - disk_paths must be an array of valid null-terminated strings
/// - disk_count must be the actual count of disk_paths
/// - disk_weights can be null (all weights default to 1)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_open_multi_disk(
    disk_paths: *const *const c_char,
    disk_weights: *const c_int,
    disk_count: c_int,
    metadata_dir: *const c_char,
    cache_size_mb: c_int,
    engine: *const c_char,
) -> *mut ProximaDBHandle {
    if disk_paths.is_null() || disk_count <= 0 {
        return ptr::null_mut();
    }

    let mut storage_locations = Vec::with_capacity(disk_count as usize);

    for i in 0..disk_count as isize {
        let path_ptr = *disk_paths.offset(i);
        if path_ptr.is_null() {
            return ptr::null_mut();
        }

        let path = match CStr::from_ptr(path_ptr).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return ptr::null_mut(),
        };

        let weight = if disk_weights.is_null() {
            1
        } else {
            *disk_weights.offset(i) as u32
        };

        storage_locations.push(StorageLocationConfig::new(path).with_weight(weight));
    }

    let metadata_path = if metadata_dir.is_null() {
        format!("{}/metadata", storage_locations[0].path)
    } else {
        match CStr::from_ptr(metadata_dir).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return ptr::null_mut(),
        }
    };

    let engine_str = if engine.is_null() {
        "sst".to_string()
    } else {
        match CStr::from_ptr(engine).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => "sst".to_string(),
        }
    };

    let config = EmbeddedConfig {
        storage_locations,
        metadata_path,
        cache_size_mb: cache_size_mb as usize,
        default_engine: engine_str,
        enable_wal: true,
        wal_sync_mode: "batch".to_string(),
        ..EmbeddedConfig::default()
    };

    match EmbeddedProximaDB::new(config) {
        Ok(db) => Box::into_raw(Box::new(ProximaDBHandle { db: Arc::new(db) })),
        Err(_) => ptr::null_mut(),
    }
}

/// Close a ProximaDB instance
///
/// # Safety
/// - handle must be a valid pointer from proximadb_open
/// - handle must not be used after this call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_close(handle: *mut ProximaDBHandle) {
    if !handle.is_null() {
        let _ = Box::from_raw(handle);
    }
}

/// Create a new collection
///
/// # Safety
/// - handle must be valid
/// - name must be a valid null-terminated string
/// - engine can be null
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_create_collection(
    handle: *mut ProximaDBHandle,
    name: *const c_char,
    dimension: c_int,
    engine: *const c_char,
) -> CResult {
    if handle.is_null() || name.is_null() {
        return error_result(1, "Invalid handle or name");
    }

    let db = &(*handle).db;
    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return error_result(2, "Invalid name string"),
    };

    let engine_opt = if engine.is_null() {
        None
    } else {
        match CStr::from_ptr(engine).to_str() {
            Ok(s) => Some(s),
            Err(_) => None,
        }
    };

    match db.create_collection(name_str, dimension as u32, engine_opt) {
        Ok(_) => success_result(),
        Err(e) => error_result(3, &format!("Failed to create collection: {}", e)),
    }
}

/// Delete a collection
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_delete_collection(
    handle: *mut ProximaDBHandle,
    name: *const c_char,
) -> CResult {
    if handle.is_null() || name.is_null() {
        return error_result(1, "Invalid handle or name");
    }

    let db = &(*handle).db;
    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return error_result(2, "Invalid name string"),
    };

    match db.delete_collection(name_str) {
        Ok(_) => success_result(),
        Err(e) => error_result(3, &format!("Failed to delete collection: {}", e)),
    }
}

/// Insert vectors into a collection
///
/// # Safety
/// - vectors is a flat array of floats: [v0_d0, v0_d1, ..., v1_d0, v1_d1, ...]
/// - ids is an array of null-terminated strings
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_insert(
    handle: *mut ProximaDBHandle,
    collection: *const c_char,
    ids: *const *const c_char,
    vectors: *const c_float,
    count: c_int,
    dimension: c_int,
) -> CResult {
    if handle.is_null() || collection.is_null() || ids.is_null() || vectors.is_null() {
        return error_result(1, "Invalid parameters");
    }

    let db = &(*handle).db;
    let collection_str = match CStr::from_ptr(collection).to_str() {
        Ok(s) => s,
        Err(_) => return error_result(2, "Invalid collection string"),
    };

    let count = count as usize;
    let dimension = dimension as usize;

    // Convert IDs
    let mut rust_ids = Vec::with_capacity(count);
    for i in 0..count {
        let id_ptr = *ids.add(i);
        if id_ptr.is_null() {
            return error_result(2, "Null ID pointer");
        }
        match CStr::from_ptr(id_ptr).to_str() {
            Ok(s) => rust_ids.push(s.to_string()),
            Err(_) => return error_result(2, "Invalid ID string"),
        }
    }

    // Convert vectors
    let mut rust_vectors = Vec::with_capacity(count);
    for i in 0..count {
        let offset = i * dimension;
        let vec_slice = std::slice::from_raw_parts(vectors.add(offset), dimension);
        rust_vectors.push(vec_slice.to_vec());
    }

    match db.insert(collection_str, rust_ids, rust_vectors, None) {
        Ok(_) => success_result(),
        Err(e) => error_result(3, &format!("Insert failed: {}", e)),
    }
}

/// Search for similar vectors
///
/// # Safety
/// - query is an array of floats with length dimension
/// - Returns CSearchResults that must be freed with proximadb_free_search_results
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_search(
    handle: *mut ProximaDBHandle,
    collection: *const c_char,
    query: *const c_float,
    dimension: c_int,
    top_k: c_int,
) -> CSearchResults {
    let empty_results = CSearchResults {
        results: ptr::null_mut(),
        count: 0,
    };

    if handle.is_null() || collection.is_null() || query.is_null() {
        return empty_results;
    }

    let db = &(*handle).db;
    let collection_str = match CStr::from_ptr(collection).to_str() {
        Ok(s) => s,
        Err(_) => return empty_results,
    };

    let query_vec = std::slice::from_raw_parts(query, dimension as usize).to_vec();

    match db.search(collection_str, query_vec, top_k as usize, None) {
        Ok(results) => {
            if results.is_empty() {
                return empty_results;
            }

            let count = results.len();
            let c_results = Box::into_raw(
                results
                    .into_iter()
                    .map(|r| {
                        let id = CString::new(r.id).unwrap_or_default().into_raw();
                        CSearchResult { id, score: r.score }
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ) as *mut CSearchResult;

            CSearchResults {
                results: c_results,
                count: count as c_int,
            }
        }
        Err(_) => empty_results,
    }
}

/// Free search results
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_free_search_results(results: CSearchResults) {
    if results.results.is_null() || results.count <= 0 {
        return;
    }

    let slice = std::slice::from_raw_parts_mut(results.results, results.count as usize);
    for result in slice.iter_mut() {
        if !result.id.is_null() {
            let _ = CString::from_raw(result.id);
        }
    }

    let _ = Box::from_raw(results.results);
}

/// Flush pending writes
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_flush(handle: *mut ProximaDBHandle) -> CResult {
    if handle.is_null() {
        return error_result(1, "Invalid handle");
    }

    let db = &(*handle).db;
    match db.flush() {
        Ok(_) => success_result(),
        Err(e) => error_result(3, &format!("Flush failed: {}", e)),
    }
}

/// Free a string allocated by this library
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}

/// Free a CResult error message
#[unsafe(no_mangle)]
pub unsafe extern "C" fn proximadb_free_result(result: CResult) {
    if !result.error_message.is_null() {
        let _ = CString::from_raw(result.error_message);
    }
}

/// Get library version
#[unsafe(no_mangle)]
pub extern "C" fn proximadb_version() -> *const c_char {
    // Return static string, no need to free
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

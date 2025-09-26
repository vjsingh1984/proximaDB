use std::path::{Path, PathBuf};

/// Storage path utilities for consistent path construction across all engines
pub struct StoragePath;

impl StoragePath {
    /// Constructs the data directory path for a collection
    /// Format: {base_url}/{collection_id}/data
    ///
    /// This is the standard location where all storage engines should store their data files.
    /// Using this helper ensures consistency across SST, VIPER, NOVA, SWIFT, RAPTOR, and HELIX engines.
    ///
    /// # Arguments
    /// * `base_url` - The base storage URL (e.g., "file:///path/to/storage" or "s3://bucket/path")
    /// * `collection_id` - The collection identifier
    ///
    /// # Returns
    /// The full path to the collection's data directory
    pub fn collection_data_path(base_url: &str, collection_id: &str) -> String {
        format!("{}/{}/data", base_url, collection_id)
    }

    /// Constructs the WAL directory path for a collection
    /// Format: {base_url}/{collection_id}/wal
    pub fn collection_wal_path(base_url: &str, collection_id: &str) -> String {
        format!("{}/{}/wal", base_url, collection_id)
    }

    /// Constructs the index directory path for a collection
    /// Format: {base_url}/{collection_id}/indexes
    pub fn collection_index_path(base_url: &str, collection_id: &str) -> String {
        format!("{}/{}/indexes", base_url, collection_id)
    }

    /// Constructs the metadata directory path for a collection
    /// Format: {base_url}/{collection_id}/metadata
    pub fn collection_metadata_path(base_url: &str, collection_id: &str) -> String {
        format!("{}/{}/metadata", base_url, collection_id)
    }

    /// Constructs the compaction staging directory path
    /// Format: {base_url}/{collection_id}/compaction_staging
    pub fn collection_compaction_staging_path(base_url: &str, collection_id: &str) -> String {
        format!("{}/{}/compaction_staging", base_url, collection_id)
    }

    /// Constructs the write buffer directory path for a collection
    /// Format: {base_url}/{collection_id}/write_buffer
    pub fn collection_write_buffer_path(base_url: &str, collection_id: &str) -> String {
        format!("{}/{}/write_buffer", base_url, collection_id)
    }

    /// Constructs a file path within the data directory
    /// Format: {base_url}/{collection_id}/data/{filename}
    pub fn data_file_path(base_url: &str, collection_id: &str, filename: &str) -> String {
        format!("{}/{}", Self::collection_data_path(base_url, collection_id), filename)
    }

    /// Parses a full path to extract the base URL and collection ID
    /// Expects format: {base_url}/{collection_id}/...
    pub fn parse_collection_path(full_path: &str) -> Option<(String, String)> {
        let path = Path::new(full_path);
        let components: Vec<_> = path.components().collect();

        // Need at least 2 components for base_url and collection_id
        if components.len() < 2 {
            return None;
        }

        // Find the collection_id (should be before /data, /wal, /indexes, etc.)
        let path_str = full_path;
        if let Some(data_pos) = path_str.rfind("/data") {
            let base_and_collection = &path_str[..data_pos];
            if let Some(last_slash) = base_and_collection.rfind('/') {
                let base_url = &base_and_collection[..last_slash];
                let collection_id = &base_and_collection[last_slash + 1..];
                return Some((base_url.to_string(), collection_id.to_string()));
            }
        }

        None
    }

    /// Ensures the path uses forward slashes consistently (important for URLs)
    pub fn normalize_path(path: &str) -> String {
        path.replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collection_data_path() {
        assert_eq!(
            StoragePath::collection_data_path("file:///storage", "test_collection"),
            "file:///storage/test_collection/data"
        );

        assert_eq!(
            StoragePath::collection_data_path("s3://bucket/path", "my-collection"),
            "s3://bucket/path/my-collection/data"
        );
    }

    #[test]
    fn test_parse_collection_path() {
        let (base, collection) = StoragePath::parse_collection_path(
            "file:///storage/test_collection/data/file.sst"
        ).unwrap();
        assert_eq!(base, "file:///storage");
        assert_eq!(collection, "test_collection");
    }

    #[test]
    fn test_data_file_path() {
        assert_eq!(
            StoragePath::data_file_path("file:///storage", "collection1", "level0_001.sst"),
            "file:///storage/collection1/data/level0_001.sst"
        );
    }
}
//! Centralized filesystem scheme validation
//!
//! This module provides a single source of truth for all filesystem scheme validation,
//! eliminating duplicate validation logic scattered across multiple modules.

use super::FilesystemError;
use url::Url;

/// Supported filesystem schemes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilesystemScheme {
    /// Local file:// scheme
    File,
    /// AWS S3 s3:// scheme
    S3,
    /// Google Cloud Storage gs:// scheme
    GoogleCloudStorage,
    /// Azure Data Lake Storage adls:// scheme
    AzureDataLakeStorage,
    /// Azure Blob Storage abfs:// scheme
    AzureBlobStorage,
    /// Hadoop HDFS hdfs:// scheme
    Hdfs,
}

impl FilesystemScheme {
    /// Get the string representation of the scheme
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::S3 => "s3",
            Self::GoogleCloudStorage => "gs",
            Self::AzureDataLakeStorage => "adls",
            Self::AzureBlobStorage => "abfs",
            Self::Hdfs => "hdfs",
        }
    }

    /// Parse a scheme string into a FilesystemScheme
    pub fn from_str(s: &str) -> Result<Self, FilesystemError> {
        match s {
            "file" => Ok(Self::File),
            "s3" => Ok(Self::S3),
            "gs" => Ok(Self::GoogleCloudStorage),
            "adls" => Ok(Self::AzureDataLakeStorage),
            "abfs" => Ok(Self::AzureBlobStorage),
            "hdfs" => Ok(Self::Hdfs),
            _ => Err(FilesystemError::UnsupportedScheme(s.to_string())),
        }
    }

    /// Get all supported schemes as strings
    pub fn all_schemes() -> &'static [&'static str] {
        &["file", "s3", "gs", "adls", "abfs", "hdfs"]
    }
}

/// Normalize a URL by adding file:// prefix if no scheme is present
pub fn normalize_url(url: &str) -> String {
    if !url.contains("://") {
        format!("file://{}", url)
    } else {
        url.to_string()
    }
}

/// Extract and validate the scheme from a URL
pub fn extract_scheme(url: &str) -> Result<FilesystemScheme, FilesystemError> {
    let normalized = normalize_url(url);
    let parsed_url = Url::parse(&normalized)
        .map_err(|e| FilesystemError::InvalidPath(format!("Invalid URL {}: {}", url, e)))?;

    FilesystemScheme::from_str(parsed_url.scheme())
}

/// Validate a URL for scheme-specific requirements
pub fn validate_url(url: &str) -> Result<(), FilesystemError> {
    let normalized = normalize_url(url);
    let parsed_url = Url::parse(&normalized)
        .map_err(|e| FilesystemError::InvalidPath(format!("Invalid URL {}: {}", url, e)))?;

    let scheme = FilesystemScheme::from_str(parsed_url.scheme())?;

    match scheme {
        FilesystemScheme::File => {
            // File URLs are valid - local filesystem
            // No additional validation needed
            Ok(())
        }
        FilesystemScheme::S3 => {
            // S3 URLs must have bucket name
            if parsed_url
                .host_str()
                .is_none_or(|host| host.is_empty())
            {
                return Err(FilesystemError::InvalidPath(
                    "S3 URLs must specify bucket name".to_string(),
                ));
            }
            Ok(())
        }
        FilesystemScheme::GoogleCloudStorage => {
            // GCS URLs must have bucket name
            if parsed_url
                .host_str()
                .is_none_or(|host| host.is_empty())
            {
                return Err(FilesystemError::InvalidPath(
                    "Google Cloud Storage URLs must specify bucket name".to_string(),
                ));
            }
            Ok(())
        }
        FilesystemScheme::AzureDataLakeStorage => {
            // ADLS URLs must have account name
            if parsed_url
                .host_str()
                .is_none_or(|host| host.is_empty())
            {
                return Err(FilesystemError::InvalidPath(
                    "Azure Data Lake Storage URLs must specify account name".to_string(),
                ));
            }
            Ok(())
        }
        FilesystemScheme::AzureBlobStorage => {
            // ABFS URLs must have account name
            if parsed_url
                .host_str()
                .is_none_or(|host| host.is_empty())
            {
                return Err(FilesystemError::InvalidPath(
                    "Azure Blob Storage URLs must specify account name".to_string(),
                ));
            }
            Ok(())
        }
        FilesystemScheme::Hdfs => {
            // HDFS URLs must have namenode host
            if parsed_url
                .host_str()
                .is_none_or(|host| host.is_empty())
            {
                return Err(FilesystemError::InvalidPath(
                    "HDFS URLs must specify namenode host".to_string(),
                ));
            }
            Ok(())
        }
    }
}

/// Check if a scheme is supported
pub fn is_supported_scheme(scheme: &str) -> bool {
    FilesystemScheme::from_str(scheme).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheme_parsing() {
        assert_eq!(
            FilesystemScheme::from_str("file").unwrap(),
            FilesystemScheme::File
        );
        assert_eq!(
            FilesystemScheme::from_str("s3").unwrap(),
            FilesystemScheme::S3
        );
        assert_eq!(
            FilesystemScheme::from_str("gs").unwrap(),
            FilesystemScheme::GoogleCloudStorage
        );
        assert!(FilesystemScheme::from_str("invalid").is_err());
    }

    #[test]
    fn test_normalize_url() {
        assert_eq!(normalize_url("/tmp/data"), "file:///tmp/data");
        assert_eq!(normalize_url("file:///tmp/data"), "file:///tmp/data");
        assert_eq!(normalize_url("s3://bucket/path"), "s3://bucket/path");
    }

    #[test]
    fn test_extract_scheme() {
        assert_eq!(extract_scheme("/tmp/data").unwrap(), FilesystemScheme::File);
        assert_eq!(
            extract_scheme("file:///tmp/data").unwrap(),
            FilesystemScheme::File
        );
        assert_eq!(
            extract_scheme("s3://bucket/path").unwrap(),
            FilesystemScheme::S3
        );
    }

    #[test]
    fn test_validate_url() {
        // Valid file URL
        assert!(validate_url("file:///tmp/data").is_ok());
        assert!(validate_url("/tmp/data").is_ok());

        // Valid S3 URL
        assert!(validate_url("s3://bucket/path").is_ok());

        // Invalid S3 URL (no bucket)
        assert!(validate_url("s3:///path").is_err());

        // Invalid scheme
        assert!(validate_url("invalid://path").is_err());
    }

    #[test]
    fn test_is_supported_scheme() {
        assert!(is_supported_scheme("file"));
        assert!(is_supported_scheme("s3"));
        assert!(is_supported_scheme("gs"));
        assert!(!is_supported_scheme("invalid"));
    }
}

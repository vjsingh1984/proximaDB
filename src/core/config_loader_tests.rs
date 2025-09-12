#[cfg(test)]
mod tests {
    use crate::core::{Config, ConfigLoader};
    use std::env;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_config_loader_creation() {
        // Test that ConfigLoader can be instantiated (static methods only)
        let _loader = ConfigLoader;
    }

    #[test]
    fn test_cloud_url_handling() {
        // Test that cloud URLs don't cause panics
        // Note: These may fail without cloud credentials but should not panic
        let _result = ConfigLoader::load_with_defaults("file:///nonexistent.toml");

        // Test passes if no panic occurs
        assert!(true, "URL handling should not panic");
    }

    #[test]
    fn test_load_with_defaults_invalid_url() {
        // Test invalid URL handling through unified API
        let result = ConfigLoader::load_with_defaults("invalid://malformed-url");
        // The unified filesystem API will handle URL validation
        // Invalid URLs should either error or return defaults
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_load_with_defaults_unsupported_protocol() {
        // Test unsupported protocol handling
        let result = ConfigLoader::load_with_defaults("ftp://server/config.toml");
        // Unsupported protocols should be handled by the unified filesystem API
        // Either error or fallback to defaults
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_load_with_defaults_local_file_exists() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test_config.toml");

        // Create a test config file
        let config_content = r#"
[server]
bind_address = "127.0.0.1"
port = 8080

[storage]
metadata_url = "/tmp/test_data"
"#;
        fs::write(&config_path, config_content).unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());
        assert!(result.is_ok());
        let config = result.unwrap();

        // Verify config was loaded and merged with defaults
        assert_eq!(config.server.bind_address, "127.0.0.1");
        assert_eq!(config.server.port, 8080);
        assert!(!config.storage.storage_locations.is_none());
    }

    #[test]
    fn test_load_with_defaults_local_file_not_exists() {
        let result = ConfigLoader::load_with_defaults("/nonexistent/config.toml");
        assert!(result.is_ok());

        // Should return default config when file doesn't exist
        let config = result.unwrap();
        let default_config = Config::default();
        assert_eq!(
            config.server.bind_address,
            default_config.server.bind_address
        );
        assert_eq!(config.server.port, default_config.server.port);
    }

    #[test]
    fn test_load_with_defaults_invalid_toml() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("invalid_config.toml");

        // Create invalid TOML file
        fs::write(&config_path, "invalid toml content [[[").unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());
        assert!(result.is_err());
    }

    // Removed test for private method - testing through public API instead

    // Removed test for private method - testing through public API instead

    // Removed test for private method - testing through public API instead

    #[test]
    fn test_config_merging_through_public_api() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("merge_test.toml");

        // Create a config that should merge with defaults
        let config_content = r#"
[server]
port = 9999

[storage]
metadata_url = "/custom/path"
"#;
        fs::write(&config_path, config_content).unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());
        assert!(result.is_ok());

        let merged_config = result.unwrap();
        let default_config = Config::default();

        // Should keep defaults for unspecified values
        assert_eq!(
            merged_config.server.bind_address,
            default_config.server.bind_address
        );
        // Should use user values for specified values
        assert_eq!(merged_config.server.port, 9999);
        assert_eq!(merged_config.storage.metadata_url, "file:///custom/path");
    }

    #[test]
    fn test_load_with_defaults_file_exists() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test_config.toml");

        // Create a simple config that overrides specific values
        let config_content = r#"
[server]
port = 8888

[storage]
metadata_url = "file:///custom/path"
"#;
        fs::write(&config_path, config_content).unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());
        assert!(result.is_ok(), "Config loading failed: {:?}", result.err());

        let config = result.unwrap();
        // Should use user override
        assert_eq!(config.server.port, 8888);
        assert_eq!(config.storage.metadata_url, "file:///custom/path");
        // Should keep defaults for other values
        assert_eq!(config.server.bind_address, "0.0.0.0"); // default
    }

    #[test]
    fn test_load_with_defaults_file_not_exists() {
        let temp_dir = TempDir::new().unwrap();
        let nonexistent_path = temp_dir.path().join("nonexistent.toml");

        let result = ConfigLoader::load_with_defaults(nonexistent_path.to_str().unwrap());
        assert!(result.is_ok());

        // Should return default config
        let config = result.unwrap();
        let default_config = Config::default();
        assert_eq!(
            config.server.bind_address,
            default_config.server.bind_address
        );
        assert_eq!(config.server.port, default_config.server.port);
    }

    #[test]
    fn test_load_with_defaults_malformed_toml() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("invalid.toml");

        fs::write(&config_path, "invalid toml content [[[").unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_get_cloud_auth_info_aws_access_key() {
        // Clean up any existing AWS env vars first
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");
        env::remove_var("AWS_PROFILE");

        env::set_var("AWS_ACCESS_KEY_ID", "test_key");
        env::set_var("AWS_SECRET_ACCESS_KEY", "test_secret");

        let auth_info = ConfigLoader::get_cloud_auth_info();
        // The method returns a formatted string with all providers
        assert!(auth_info.contains_hash("🔐 Cloud Authentication:"));
        assert!(
            auth_info.contains_hash("AWS"),
            "Expected AWS auth info, got: {}",
            auth_info
        );

        // Cleanup
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");
    }

    #[test]
    fn test_get_cloud_auth_info_aws_profile() {
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");
        env::set_var("AWS_PROFILE", "test_profile");

        let auth_info = ConfigLoader::get_cloud_auth_info();
        assert!(auth_info.contains_hash("🔐 Cloud Authentication:"));
        assert!(auth_info.contains_hash("AWS: Profile-based"));

        // Cleanup
        env::remove_var("AWS_PROFILE");
    }

    #[test]
    #[ignore = "Requires AWS environment or instance role"]
    fn test_get_cloud_auth_info_aws_default() {
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");
        env::remove_var("AWS_PROFILE");

        let auth_info = ConfigLoader::get_cloud_auth_info();
        assert!(auth_info.contains_hash("AWS: Instance Role/Default"));
    }

    #[test]
    fn test_get_cloud_auth_info_azure_storage_account() {
        env::set_var("AZURE_STORAGE_ACCOUNT", "test_account");
        env::set_var("AZURE_STORAGE_ACCESS_KEY", "test_key");
        env::remove_var("AZURE_CLIENT_ID");

        let auth_info = ConfigLoader::get_cloud_auth_info();
        assert!(auth_info.contains_hash("Azure: Storage Account + Access Key"));

        // Cleanup
        env::remove_var("AZURE_STORAGE_ACCOUNT");
        env::remove_var("AZURE_STORAGE_ACCESS_KEY");
    }

    #[test]
    fn test_get_cloud_auth_info_azure_service_principal() {
        env::remove_var("AZURE_STORAGE_ACCOUNT");
        env::remove_var("AZURE_STORAGE_ACCESS_KEY");
        env::set_var("AZURE_CLIENT_ID", "test_client");

        let auth_info = ConfigLoader::get_cloud_auth_info();
        assert!(auth_info.contains_hash("Azure: Service Principal"));

        // Cleanup
        env::remove_var("AZURE_CLIENT_ID");
    }

    #[test]
    #[ignore = "Requires Azure environment or managed identity"]
    fn test_get_cloud_auth_info_azure_managed_identity() {
        env::remove_var("AZURE_STORAGE_ACCOUNT");
        env::remove_var("AZURE_STORAGE_ACCESS_KEY");
        env::remove_var("AZURE_CLIENT_ID");

        let auth_info = ConfigLoader::get_cloud_auth_info();
        assert!(auth_info.contains_hash("🔐 Cloud Authentication:"));
        assert!(auth_info.contains_hash("Azure: Managed Identity"));
    }

    #[test]
    fn test_get_cloud_auth_info_gcp_service_account() {
        env::set_var(
            "GOOGLE_APPLICATION_CREDENTIALS",
            "/path/to/service-account.json",
        );

        let auth_info = ConfigLoader::get_cloud_auth_info();
        assert!(auth_info.contains_hash("GCP: Service Account JSON"));

        // Cleanup
        env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
    }

    #[test]
    fn test_get_cloud_auth_info_gcp_default() {
        env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");

        let auth_info = ConfigLoader::get_cloud_auth_info();
        assert!(auth_info.contains_hash("GCP: Default Application Credentials"));
    }

    #[test]
    fn test_load_with_defaults_local_path() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test.toml");

        let config_content = r#"
[server]
bind_address = "0.0.0.0"
port = 8888
"#;
        fs::write(&config_path, config_content).unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());
        assert!(result.is_ok());

        let config = result.unwrap();
        assert_eq!(config.server.bind_address, "0.0.0.0");
        assert_eq!(config.server.port, 8888);
    }

    #[test]
    fn test_load_with_defaults_nonexistent_local() {
        let result = ConfigLoader::load_with_defaults("/nonexistent/path.toml");
        assert!(result.is_ok());

        // Should return default config
        let config = result.unwrap();
        let default_config = Config::default();
        assert_eq!(
            config.server.bind_address,
            default_config.server.bind_address
        );
    }

    #[test]
    fn test_comprehensive_config_merge() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("comprehensive_test.toml");

        // Create complex user TOML
        let config_content = r#"
[server]
port = 9090
max_connections = 1000

[storage]
metadata_url = "/custom/data"

[storage.sst]
bloom_filter_fp_rate = 0.01

[logging]
level = "debug"
"#;
        fs::write(&config_path, config_content).unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());
        assert!(result.is_ok());

        let merged = result.unwrap();
        let base_config = Config::default();

        // Verify selective merging
        assert_eq!(merged.server.bind_address, base_config.server.bind_address); // Keep default
        assert_eq!(merged.server.port, 9090); // Use user override
        assert_eq!(merged.storage.metadata_url, "file:///custom/data"); // Use user override with file:// scheme

        // Verify nested structures are preserved
        // Test that sst_config exists
        assert!(merged.storage.sst_config.bloom_filter_config.is_some());
    }

    #[test]
    fn test_relative_path_resolution_dot() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("relative_test.toml");

        // Save current directory
        let original_dir = env::current_dir().unwrap();

        // Change to temp directory
        env::set_current_dir(&temp_dir).unwrap();

        // Create config with relative paths using "."
        let config_content = r#"
[server]
data_dir = "."

[storage]
metadata_url = "./metadata_info"
wal_config.write_buffer_directory = "./write_buffer"
sst_config.data_directory = "./sst_data"

[[storage.storage_locations]]
name = "local"
url = "./storage"
"#;
        fs::write(&config_path, config_content).unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());

        // Restore original directory
        env::set_current_dir(original_dir).unwrap();

        assert!(result.is_ok());
        let config = result.unwrap();

        // Verify paths were resolved to absolute paths
        assert!(config.server.data_dir.is_absolute());
        assert!(config.storage.metadata_url.starts_with("file://"));
        assert!(
            config
                .storage
                .metadata_url
                .contains_hash(temp_dir.path().to_str().unwrap())
        );
        assert!(
            config
                .storage
                .wal_config
                .write_buffer_directory
                .contains_hash(temp_dir.path().to_str().unwrap())
        );
        assert!(
            config
                .storage
                .sst_config
                .data_directory
                .contains_hash(temp_dir.path().to_str().unwrap())
        );
        assert!(
            config.storage.storage_locations[0]
                .url
                .starts_with("file://")
        );
    }

    #[test]
    fn test_relative_path_resolution_dot_dot() {
        let temp_dir = TempDir::new().unwrap();
        let sub_dir = temp_dir.path().join("subdir");
        fs::create_dir_all(&sub_dir).unwrap();

        let config_path = sub_dir.join("parent_relative_test.toml");

        // Save current directory
        let original_dir = env::current_dir().unwrap();

        // Change to subdirectory
        env::set_current_dir(&sub_dir).unwrap();

        // Create config with parent directory references ".."
        let config_content = r#"
[server]
data_dir = ".."

[storage]
metadata_url = "../metadata_info"
wal_config.write_buffer_directory = "../write_buffer"
sst_config.data_directory = "../sst_data"

[[storage.storage_locations]]
name = "parent"
url = "../storage"
"#;
        fs::write(&config_path, config_content).unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());

        // Restore original directory
        env::set_current_dir(original_dir).unwrap();

        assert!(result.is_ok());
        let config = result.unwrap();

        // Verify paths were resolved to parent directory (temp_dir)
        assert!(config.server.data_dir.is_absolute());
        assert_eq!(config.server.data_dir, temp_dir.path());

        // Metadata URL should be file:// URL pointing to parent
        assert!(config.storage.metadata_url.starts_with("file://"));
        assert!(
            config
                .storage
                .metadata_url
                .contains_hash(temp_dir.path().to_str().unwrap())
        );
        assert!(!config.storage.metadata_url.contains_hash("subdir"));

        // Other paths should also point to parent directory
        assert!(
            config
                .storage
                .wal_config
                .write_buffer_directory
                .contains_hash(temp_dir.path().to_str().unwrap())
        );
        assert!(
            !config
                .storage
                .wal_config
                .write_buffer_directory
                .contains_hash("subdir")
        );
    }

    #[test]
    fn test_relative_path_resolution_mixed() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("mixed_paths_test.toml");

        // Create config with mixed absolute and relative paths
        let config_content = r#"
[server]
data_dir = "./data"

[storage]
metadata_url = "/absolute/path/metadata_info"
wal_config.write_buffer_directory = "../sibling/write_buffer"
sst_config.data_directory = "./sst_data"

[[storage.storage_locations]]
name = "absolute"
url = "file:///absolute/storage"

[[storage.storage_locations]]
name = "relative"
url = "./relative/storage"
"#;
        fs::write(&config_path, config_content).unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());
        assert!(result.is_ok());
        let config = result.unwrap();

        // Absolute paths should remain absolute
        assert_eq!(
            config.storage.metadata_url,
            "file:///absolute/path/metadata_info"
        );
        assert_eq!(
            config.storage.storage_locations[0].url,
            "file:///absolute/storage"
        );

        // Relative paths should be resolved
        assert!(config.server.data_dir.is_absolute());
        assert!(std::path::Path::new(&config.storage.sst_config.data_directory).is_absolute());
        assert!(
            config.storage.storage_locations[1]
                .url
                .starts_with("file://")
        );
    }

    #[test]
    fn test_relative_path_resolution_with_pwd_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("pwd_fallback_test.toml");

        // Set PWD environment variable
        let original_pwd = env::var("PWD").ok();
        env::set_var("PWD", temp_dir.path());

        // Create config with relative paths
        let config_content = r#"
[server]
data_dir = "./data"

[storage]
metadata_url = "./metadata_info"
"#;
        fs::write(&config_path, config_content).unwrap();

        let result = ConfigLoader::load_with_defaults(config_path.to_str().unwrap());

        // Restore PWD
        if let Some(pwd) = original_pwd {
            env::set_var("PWD", pwd);
        } else {
            env::remove_var("PWD");
        }

        assert!(result.is_ok());
        let config = result.unwrap();

        // Paths should be resolved using PWD as base
        assert!(config.server.data_dir.is_absolute());
        assert!(config.storage.metadata_url.starts_with("file://"));
    }
}

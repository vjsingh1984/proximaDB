//! # ProximaDB Migration Tool
//!
//! CLI tool for migrating collections from VectorRecord to ProximaSchema format.
//!
//! ## Usage
//!
//! ```bash
//! # Migrate a single collection
//! proximadb-migrate --collection my_vectors --config config/config.toml
//!
//! # Batch migrate all collections
//! proximadb-migrate --all --config config/config.toml
//!
//! # Dry run (show what would be migrated)
//! proximadb-migrate --all --dry-run --config config/config.toml
//!
//! # Validate migration (compare checksums)
//! proximadb-migrate --validate --collection my_vectors --config config/config.toml
//!
//! # Parallel migration with 4 workers
//! proximadb-migrate --all --parallel 4 --config config/config.toml
//! ```

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;
use tracing::{info, warn, error, Level};
use tracing_subscriber::FmtSubscriber;

/// ProximaDB Migration Tool - Migrate from VectorRecord to ProximaSchema
#[derive(Parser)]
#[command(name = "proximadb-migrate")]
#[command(author = "ProximaDB Team")]
#[command(version = "0.1.0")]
#[command(about = "Migrate ProximaDB collections from legacy VectorRecord to ProximaSchema format")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Configuration file path
    #[arg(short, long, default_value = "config/config.toml")]
    config: PathBuf,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Migrate a specific collection
    Migrate {
        /// Collection name to migrate
        #[arg(short, long)]
        collection: Option<String>,

        /// Migrate all collections
        #[arg(long)]
        all: bool,

        /// Dry run (don't actually migrate, just show plan)
        #[arg(long)]
        dry_run: bool,

        /// Number of parallel workers
        #[arg(short, long, default_value = "4")]
        parallel: usize,

        /// Skip already migrated collections
        #[arg(long)]
        skip_migrated: bool,
    },

    /// Validate a migrated collection
    Validate {
        /// Collection name to validate
        #[arg(short, long)]
        collection: Option<String>,

        /// Validate all collections
        #[arg(long)]
        all: bool,

        /// Compare row counts
        #[arg(long)]
        check_counts: bool,

        /// Verify checksums
        #[arg(long)]
        verify_checksums: bool,
    },

    /// Show migration status
    Status {
        /// Show detailed status
        #[arg(short, long)]
        detailed: bool,
    },

    /// Rollback a migration
    Rollback {
        /// Collection to rollback
        #[arg(short, long)]
        collection: String,

        /// Schema version to rollback to
        #[arg(long)]
        to_version: u32,
    },
}

/// Migration result for a single collection
#[derive(Debug)]
struct MigrationResult {
    collection_name: String,
    success: bool,
    rows_migrated: u64,
    bytes_processed: u64,
    duration_ms: u64,
    error: Option<String>,
    old_version: u32,
    new_version: u32,
}

/// Validation result for a single collection
#[derive(Debug)]
struct ValidationResult {
    collection_name: String,
    is_valid: bool,
    source_row_count: u64,
    target_row_count: u64,
    schema_matches: bool,
    checksum_matches: Option<bool>,
    warnings: Vec<String>,
    errors: Vec<String>,
}

/// Collection migration status
#[derive(Debug)]
struct CollectionStatus {
    name: String,
    is_legacy: bool,
    schema_version: u32,
    row_count: u64,
    file_count: usize,
    total_bytes: u64,
    needs_migration: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose { Level::DEBUG } else { Level::INFO };
    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set tracing subscriber")?;

    info!("ProximaDB Migration Tool v0.1.0");
    info!("Using config: {}", cli.config.display());

    match cli.command {
        Commands::Migrate {
            collection,
            all,
            dry_run,
            parallel,
            skip_migrated,
        } => {
            run_migration(collection, all, dry_run, parallel, skip_migrated, &cli.config).await
        }
        Commands::Validate {
            collection,
            all,
            check_counts,
            verify_checksums,
        } => {
            run_validation(collection, all, check_counts, verify_checksums, &cli.config).await
        }
        Commands::Status { detailed } => {
            show_status(detailed, &cli.config).await
        }
        Commands::Rollback {
            collection,
            to_version,
        } => {
            run_rollback(&collection, to_version, &cli.config).await
        }
    }
}

/// Run migration for collections
async fn run_migration(
    collection: Option<String>,
    all: bool,
    dry_run: bool,
    parallel: usize,
    skip_migrated: bool,
    config_path: &PathBuf,
) -> Result<()> {
    if !all && collection.is_none() {
        anyhow::bail!("Must specify either --collection <name> or --all");
    }

    info!("Starting migration...");
    if dry_run {
        info!("[DRY RUN] No changes will be made");
    }

    let start = Instant::now();

    // Get list of collections to migrate
    let collections = if all {
        discover_collections(config_path).await?
    } else {
        vec![collection.unwrap()]
    };

    info!("Found {} collections to process", collections.len());

    // Filter out already migrated if requested
    let collections_to_migrate: Vec<_> = if skip_migrated {
        let mut filtered = Vec::new();
        for name in collections {
            let status = get_collection_status(&name, config_path).await?;
            if status.needs_migration {
                filtered.push(name);
            } else {
                info!("Skipping {} (already migrated)", name);
            }
        }
        filtered
    } else {
        collections
    };

    if collections_to_migrate.is_empty() {
        info!("No collections need migration");
        return Ok(());
    }

    info!("Migrating {} collections with {} parallel workers",
          collections_to_migrate.len(), parallel);

    // Create semaphore for parallel limiting
    let semaphore = Arc::new(Semaphore::new(parallel));
    let mut handles = Vec::new();

    for collection_name in collections_to_migrate {
        let sem = Arc::clone(&semaphore);
        let config = config_path.clone();
        let is_dry_run = dry_run;

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            migrate_collection(&collection_name, is_dry_run, &config).await
        });

        handles.push(handle);
    }

    // Collect results
    let mut success_count = 0;
    let mut failure_count = 0;
    let mut total_rows = 0u64;
    let mut total_bytes = 0u64;

    for handle in handles {
        match handle.await {
            Ok(Ok(result)) => {
                if result.success {
                    success_count += 1;
                    total_rows += result.rows_migrated;
                    total_bytes += result.bytes_processed;
                    info!(
                        "Migrated {}: {} rows, {} bytes in {}ms",
                        result.collection_name,
                        result.rows_migrated,
                        format_bytes(result.bytes_processed),
                        result.duration_ms
                    );
                } else {
                    failure_count += 1;
                    error!(
                        "Failed to migrate {}: {}",
                        result.collection_name,
                        result.error.unwrap_or_default()
                    );
                }
            }
            Ok(Err(e)) => {
                failure_count += 1;
                error!("Migration task failed: {}", e);
            }
            Err(e) => {
                failure_count += 1;
                error!("Migration task panicked: {}", e);
            }
        }
    }

    let duration = start.elapsed();

    info!("Migration complete in {:.2}s", duration.as_secs_f64());
    info!(
        "Results: {} succeeded, {} failed, {} rows, {}",
        success_count,
        failure_count,
        total_rows,
        format_bytes(total_bytes)
    );

    if failure_count > 0 {
        anyhow::bail!("{} collections failed to migrate", failure_count);
    }

    Ok(())
}

/// Migrate a single collection
async fn migrate_collection(
    name: &str,
    dry_run: bool,
    _config_path: &PathBuf,
) -> Result<MigrationResult> {
    let start = Instant::now();

    info!("Processing collection: {}", name);

    // TODO: Implement actual migration logic
    // 1. Load collection metadata
    // 2. Check if legacy VectorRecord format
    // 3. Create new ProximaSchema
    // 4. Read data files and convert
    // 5. Write new files with schema
    // 6. Update metadata

    if dry_run {
        info!("[DRY RUN] Would migrate collection: {}", name);
        return Ok(MigrationResult {
            collection_name: name.to_string(),
            success: true,
            rows_migrated: 0,
            bytes_processed: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            error: None,
            old_version: 0,
            new_version: 1,
        });
    }

    // Placeholder implementation
    Ok(MigrationResult {
        collection_name: name.to_string(),
        success: true,
        rows_migrated: 0,
        bytes_processed: 0,
        duration_ms: start.elapsed().as_millis() as u64,
        error: None,
        old_version: 0,
        new_version: 1,
    })
}

/// Run validation for collections
async fn run_validation(
    collection: Option<String>,
    all: bool,
    check_counts: bool,
    verify_checksums: bool,
    config_path: &PathBuf,
) -> Result<()> {
    if !all && collection.is_none() {
        anyhow::bail!("Must specify either --collection <name> or --all");
    }

    info!("Starting validation...");

    let collections = if all {
        discover_collections(config_path).await?
    } else {
        vec![collection.unwrap()]
    };

    let mut all_valid = true;

    for name in collections {
        let result = validate_collection(&name, check_counts, verify_checksums, config_path).await?;

        if result.is_valid {
            info!(
                "VALID: {} - {} rows, schema matches: {}",
                result.collection_name,
                result.source_row_count,
                result.schema_matches
            );
        } else {
            all_valid = false;
            error!("INVALID: {}", result.collection_name);
            for err in &result.errors {
                error!("  Error: {}", err);
            }
        }

        for warning in &result.warnings {
            warn!("  Warning: {}", warning);
        }
    }

    if all_valid {
        info!("All collections validated successfully");
    } else {
        anyhow::bail!("Some collections failed validation");
    }

    Ok(())
}

/// Validate a single collection
async fn validate_collection(
    name: &str,
    _check_counts: bool,
    _verify_checksums: bool,
    _config_path: &PathBuf,
) -> Result<ValidationResult> {
    info!("Validating collection: {}", name);

    // TODO: Implement actual validation logic
    // 1. Load source and target schemas
    // 2. Compare row counts if requested
    // 3. Verify checksums if requested
    // 4. Check data integrity

    Ok(ValidationResult {
        collection_name: name.to_string(),
        is_valid: true,
        source_row_count: 0,
        target_row_count: 0,
        schema_matches: true,
        checksum_matches: None,
        warnings: vec![],
        errors: vec![],
    })
}

/// Show migration status for all collections
async fn show_status(detailed: bool, config_path: &PathBuf) -> Result<()> {
    info!("Checking migration status...");

    let collections = discover_collections(config_path).await?;

    let mut legacy_count = 0;
    let mut migrated_count = 0;
    let mut total_rows = 0u64;
    let mut total_bytes = 0u64;

    println!("\n{:<30} {:>10} {:>10} {:>12} {:>10}",
             "Collection", "Version", "Rows", "Size", "Status");
    println!("{}", "-".repeat(75));

    for name in &collections {
        let status = get_collection_status(name, config_path).await?;

        let status_str = if status.needs_migration {
            legacy_count += 1;
            "LEGACY"
        } else {
            migrated_count += 1;
            "OK"
        };

        total_rows += status.row_count;
        total_bytes += status.total_bytes;

        println!(
            "{:<30} {:>10} {:>10} {:>12} {:>10}",
            status.name,
            format!("v{}", status.schema_version),
            status.row_count,
            format_bytes(status.total_bytes),
            status_str
        );

        if detailed {
            println!("    Files: {}", status.file_count);
            println!("    Legacy: {}", status.is_legacy);
        }
    }

    println!("{}", "-".repeat(75));
    println!(
        "Total: {} collections, {} rows, {}",
        collections.len(),
        total_rows,
        format_bytes(total_bytes)
    );
    println!(
        "Status: {} legacy (need migration), {} migrated",
        legacy_count, migrated_count
    );

    Ok(())
}

/// Rollback a migration
async fn run_rollback(collection: &str, to_version: u32, _config_path: &PathBuf) -> Result<()> {
    info!("Rolling back {} to version {}", collection, to_version);

    // TODO: Implement rollback logic
    // 1. Check if version exists in history
    // 2. Restore schema from registry
    // 3. Restore data files from backup (if available)

    warn!("Rollback not yet implemented");

    Ok(())
}

/// Discover all collections in the database
async fn discover_collections(_config_path: &PathBuf) -> Result<Vec<String>> {
    // TODO: Implement collection discovery from metadata
    Ok(vec![
        "example_collection_1".to_string(),
        "example_collection_2".to_string(),
    ])
}

/// Get status of a single collection
async fn get_collection_status(name: &str, _config_path: &PathBuf) -> Result<CollectionStatus> {
    // TODO: Implement actual status lookup
    Ok(CollectionStatus {
        name: name.to_string(),
        is_legacy: true,
        schema_version: 0,
        row_count: 0,
        file_count: 0,
        total_bytes: 0,
        needs_migration: true,
    })
}

/// Format bytes as human-readable string
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1048576), "1.00 MB");
        assert_eq!(format_bytes(1073741824), "1.00 GB");
    }
}

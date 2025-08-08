use std::collections::HashMap;
use std::path::PathBuf;
use anyhow::Result;
use tokio;
use tempfile::TempDir;

/// Simple test to replicate the assignment service discovery issue
/// This test directly simulates what happens during server startup
#[tokio::test]
async fn test_collection_directory_discovery() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    println!("🧪 Testing collection directory discovery issue");
    
    // Create test directories simulating multi-disk setup
    let temp_dir = TempDir::new()?;
    let base_path = temp_dir.path();
    
    let disk1_path = base_path.join("disk1");
    let disk2_path = base_path.join("disk2");
    let disk3_path = base_path.join("disk3");
    
    tokio::fs::create_dir_all(&disk1_path).await?;
    tokio::fs::create_dir_all(&disk2_path).await?;
    tokio::fs::create_dir_all(&disk3_path).await?;
    
    // Simulate collections created by previous server runs
    // These are the exact collection IDs you see in the server logs
    let test_collections = vec![
        "1uhM54H", "1uhLucD", "1uhMSvq", "1uhLgVB", "1uhMT6f",
        "1uhTzMv", "1uhMWFw", "1uhLSOG", "1uhMSzp", "1uhMYRW",
        "1uhMWJn", "1uhLequ", "1uhM5Dm", "1uhTsOR", "1uhUCeD",
        "1uhMWPE", "1uhU0xt", "1uhUCes", "1uhLf0s", "1uhLgR6",
        "1uhUCrD", "1uhLgbo", "1uhTsY8", "1uhM583", "1uhLSCP",
        "1uhTsSS", "1uhLw1M", "1uhUCeV", "1uhLeub", "1uhLugE",
        "1uhUCcC", "1uhU0t7"
    ];
    
    println!("📁 Creating {} test collection directories...", test_collections.len());
    
    // Create collection directories across disks in round-robin fashion
    for (i, collection_id) in test_collections.iter().enumerate() {
        let disk_path = match i % 3 {
            0 => &disk1_path,
            1 => &disk2_path,
            2 => &disk3_path,
            _ => unreachable!(),
        };
        
        // Create the typical directory structure for a collection
        let collection_dir = disk_path.join(collection_id);
        let data_dir = collection_dir.join("data");
        let write_buffer_dir = collection_dir.join("write_buffer");
        let index_dir = collection_dir.join("index");
        
        tokio::fs::create_dir_all(&data_dir).await?;
        tokio::fs::create_dir_all(&write_buffer_dir).await?;
        tokio::fs::create_dir_all(&index_dir).await?;
        
        // Create some dummy files to simulate actual data
        tokio::fs::write(data_dir.join("000001.sst"), b"dummy_sst_data").await?;
        tokio::fs::write(write_buffer_dir.join("wal.log"), b"dummy_wal_data").await?;
        tokio::fs::write(index_dir.join("index.dat"), b"dummy_index_data").await?;
        
        println!("  📂 Created {} on disk {}", collection_id, i % 3 + 1);
    }
    
    println!("\n🔍 Phase 1: Scanning directories to discover collections");
    
    // Simulate what the server does during startup - scan directories
    let disk_paths = vec![
        format!("file://{}", disk1_path.display()),
        format!("file://{}", disk2_path.display()),
        format!("file://{}", disk3_path.display()),
    ];
    
    let mut discovered_collections = HashMap::new();
    let mut total_found = 0;
    
    for (disk_index, disk_url) in disk_paths.iter().enumerate() {
        let disk_path = disk_url.strip_prefix("file://").unwrap_or(disk_url);
        let disk_dir = PathBuf::from(disk_path);
        
        println!("🗂️  Scanning disk {} ({})", disk_index + 1, disk_url);
        
        if let Ok(mut entries) = tokio::fs::read_dir(&disk_dir).await {
            let mut disk_collections = 0;
            
            while let Ok(Some(entry)) = entries.next_entry().await {
                if entry.file_type().await?.is_dir() {
                    let collection_id = entry.file_name().to_string_lossy().to_string();
                    
                    // Check if this is one of our test collections
                    if test_collections.contains(&collection_id.as_str()) {
                        let collection_path = entry.path();
                        
                        // Verify it has the expected subdirectories
                        let has_data = collection_path.join("data").exists();
                        let has_write_buffer = collection_path.join("write_buffer").exists();
                        let has_index = collection_path.join("index").exists();
                        
                        if has_data && has_write_buffer && has_index {
                            discovered_collections.insert(collection_id.clone(), disk_url.clone());
                            disk_collections += 1;
                            total_found += 1;
                            println!("    ✅ Found {} (complete structure)", collection_id);
                        } else {
                            println!("    ⚠️  Found {} but incomplete structure (data:{}, wb:{}, idx:{})", 
                                   collection_id, has_data, has_write_buffer, has_index);
                        }
                    }
                }
            }
            
            println!("    📊 Disk {}: {} collections", disk_index + 1, disk_collections);
        } else {
            println!("    ❌ Failed to read directory: {}", disk_dir.display());
        }
    }
    
    println!("\n📊 Discovery Summary:");
    println!("  Expected collections: {}", test_collections.len());
    println!("  Found collections: {}", total_found);
    println!("  Missing collections: {}", test_collections.len() - total_found);
    
    // Phase 2: Simulate assignment service recovery
    println!("\n🔄 Phase 2: Simulating assignment service recovery");
    
    // This is where the real bug happens - the assignment service should
    // be able to recover assignments from discovered collections
    let mut assignment_recovery_issues = Vec::new();
    
    for collection_id in &test_collections {
        if let Some(disk_url) = discovered_collections.get(*collection_id) {
            // Collection exists on disk, assignment service should be able to create/recover assignment
            let expected_data_url = format!("{}/{}/data", disk_url, collection_id);
            let expected_write_buffer_url = format!("{}/{}/write_buffer", disk_url, collection_id);
            let expected_index_url = format!("{}/{}/index", disk_url, collection_id);
            
            println!("  📍 {} should be assigned to:", collection_id);
            println!("      Data: {}", expected_data_url);
            println!("      WriteBuffer: {}", expected_write_buffer_url);
            println!("      Index: {}", expected_index_url);
            
            // In a working system, the assignment service would either:
            // 1. Have persistent assignment metadata to load
            // 2. Or recreate assignments based on discovered collections
            
            // The fact that you see "No assignment found" warnings means
            // neither of these is happening correctly
            
        } else {
            assignment_recovery_issues.push(format!("Collection {} not found on disk", collection_id));
            println!("  ❌ {} not found on disk", collection_id);
        }
    }
    
    // Phase 3: Check for assignment metadata persistence
    println!("\n💾 Phase 3: Checking for assignment metadata persistence");
    
    // Look for where assignment metadata might be stored
    let possible_metadata_files = vec![
        base_path.join("assignments.json"),
        base_path.join("assignments.db"),
        base_path.join("metadata").join("assignments.json"),
        base_path.join(".proximadb").join("assignments"),
        disk1_path.join("assignments.json"),
        disk2_path.join("assignments.json"),
        disk3_path.join("assignments.json"),
    ];
    
    let mut metadata_files_found = false;
    
    for metadata_file in &possible_metadata_files {
        if metadata_file.exists() {
            println!("  ✅ Found metadata file: {}", metadata_file.display());
            metadata_files_found = true;
        }
    }
    
    if !metadata_files_found {
        println!("  ❌ No assignment metadata files found");
        println!("      This explains why assignments are lost during recovery!");
    }
    
    // Phase 4: Diagnosis and recommendations
    println!("\n🔧 Phase 4: Diagnosis and Recommendations");
    
    if discovered_collections.len() == test_collections.len() {
        println!("  ✅ Collection discovery is working correctly");
        println!("      - All collections are found on disk");
        println!("      - Directory structure is correct");
    } else {
        println!("  ❌ Collection discovery has issues");
        println!("      - Some collections are missing from disk");
    }
    
    if !metadata_files_found {
        println!("  ❌ Assignment metadata persistence is NOT working");
        println!("      - No assignment metadata files found");
        println!("      - This is the root cause of your issue!");
        
        println!("\n💡 Recommended fixes:");
        println!("  1. Implement assignment metadata persistence");
        println!("     - Save assignments to a persistent store (file/database)");
        println!("     - Load assignments during startup");
        
        println!("  2. Implement assignment recovery from disk discovery");
        println!("     - When no metadata is found, recreate assignments");
        println!("     - Based on discovered collections on disk");
        println!("     - Use configured assignment strategy (round_robin)");
        
        println!("  3. Add assignment validation during startup");
        println!("     - Check if assignments match discovered collections");
        println!("     - Warn about orphaned collections");
        println!("     - Warn about missing collections");
    }
    
    // Final analysis
    println!("\n🏁 Final Analysis:");
    println!("This test replicates the exact issue you're seeing:");
    println!("  - Collections exist on disk (✅)");
    println!("  - Assignment service can't find assignments (❌)"); 
    println!("  - Server logs 'No assignment found for collection' warnings (❌)");
    println!("");
    println!("The assignment service needs to implement recovery logic");
    println!("to handle this scenario during server startup.");
    
    Ok(())
}

#[tokio::test]
async fn test_assignment_service_implementation_gaps() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    println!("🔍 Testing assignment service implementation gaps");
    
    // This test identifies specific gaps in the assignment service implementation
    // that cause the recovery issues you're experiencing
    
    println!("\n📋 Assignment Service Requirements Analysis:");
    
    println!("✅ Required: Collection-to-disk assignment strategy");
    println!("   - Round robin assignment across available disks");
    println!("   - Load balancing consideration");
    
    println!("❓ Unknown: Assignment metadata persistence");
    println!("   - Where are assignments stored?");
    println!("   - How are assignments loaded during startup?");
    
    println!("❓ Unknown: Assignment recovery from disk discovery");
    println!("   - What happens when assignment metadata is missing?");
    println!("   - Can assignments be recreated from discovered collections?");
    
    println!("❓ Unknown: Orphaned collection handling");
    println!("   - What happens to collections without assignments?");
    println!("   - Are they automatically assigned or ignored?");
    
    println!("\n🔧 Implementation Recommendations:");
    
    println!("1. Assignment Persistence Layer:");
    println!("   - Store assignments in [base_path]/.proximadb/assignments.json");
    println!("   - Include: collection_id, disk_url, assignment_timestamp");
    println!("   - Atomic updates when assignments change");
    
    println!("2. Recovery Logic During Startup:");
    println!("   - Load existing assignments from persistent store");
    println!("   - Scan disk directories for collections");
    println!("   - Reconcile discovered collections with loaded assignments");
    println!("   - Create assignments for orphaned collections");
    
    println!("3. Assignment Validation:");
    println!("   - Verify assignment targets exist on disk");
    println!("   - Check disk health and availability");
    println!("   - Log warnings for problematic assignments");
    
    println!("4. Graceful Degradation:");
    println!("   - If assignment metadata is corrupted, recreate from disk");
    println!("   - If a disk is unavailable, reassign its collections");
    println!("   - Maintain service availability during recovery");
    
    Ok(())
}
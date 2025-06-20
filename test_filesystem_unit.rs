use std::path::PathBuf;
use tempfile::TempDir;
use tokio;

// Import necessary modules for the test
use proximadb::storage::filesystem::{FilesystemFactory, FilesystemConfig, FileOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧪 Testing LocalFileSystem directly for file write operations");
    
    // Create temporary directory for test
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();
    
    println!("📁 Test directory: {}", temp_path.display());
    
    // Create filesystem factory
    let filesystem_config = FilesystemConfig::default();
    let filesystem_factory = FilesystemFactory::new(filesystem_config).await?;
    
    // Test with file:// URL
    let file_url = format!("file://{}", temp_path.display());
    println!("🔗 Testing filesystem URL: {}", file_url);
    
    // Get filesystem instance
    let fs = filesystem_factory.get_filesystem(&file_url)?;
    println!("✅ Filesystem instance created");
    
    // Test 1: Create directories
    println!("\n1️⃣ Testing directory creation...");
    let metadata_dir = "metadata";
    let snapshots_dir = "metadata/snapshots";
    let incremental_dir = "metadata/incremental";
    
    fs.create_dir(metadata_dir).await?;
    println!("   ✅ Created {}", metadata_dir);
    
    fs.create_dir(snapshots_dir).await?;
    println!("   ✅ Created {}", snapshots_dir);
    
    fs.create_dir(incremental_dir).await?;
    println!("   ✅ Created {}", incremental_dir);
    
    // Test 2: Write a file
    println!("\n2️⃣ Testing file write...");
    let test_file = "metadata/incremental/test_op_00000001_20250619120000.avro";
    let test_data = b"Hello, world! This is test Avro data.";
    
    let file_options = Some(FileOptions {
        create_dirs: true,
        overwrite: true,
        ..Default::default()
    });
    
    fs.write(test_file, test_data, file_options).await?;
    println!("   ✅ Wrote {} bytes to {}", test_data.len(), test_file);
    
    // Test 3: Check if file exists
    println!("\n3️⃣ Testing file existence...");
    let exists = fs.exists(test_file).await?;
    println!("   📄 File exists: {}", exists);
    
    if exists {
        // Test 4: Read file back
        println!("\n4️⃣ Testing file read...");
        let read_data = fs.read(test_file).await?;
        println!("   📖 Read {} bytes from {}", read_data.len(), test_file);
        
        if read_data == test_data {
            println!("   ✅ Data matches!");
        } else {
            println!("   ❌ Data mismatch!");
            println!("      Expected: {:?}", std::str::from_utf8(test_data));
            println!("      Got:      {:?}", std::str::from_utf8(&read_data));
        }
    }
    
    // Test 5: List files in directory
    println!("\n5️⃣ Testing directory listing...");
    let entries = fs.list(incremental_dir).await?;
    println!("   📁 Found {} entries in {}", entries.len(), incremental_dir);
    for entry in &entries {
        println!("      - {} ({} bytes, {})", 
                 entry.name, 
                 entry.metadata.size, 
                 if entry.metadata.is_directory { "DIR" } else { "FILE" });
    }
    
    // Test 6: Atomic write operations (temp + move)
    println!("\n6️⃣ Testing atomic write operations...");
    let final_file = "metadata/incremental/atomic_test.avro";
    let temp_file = "metadata/incremental/atomic_test.avro.tmp";
    let atomic_data = b"This is atomic test data for Avro operations.";
    
    // Write to temp file
    fs.write(temp_file, atomic_data, Some(FileOptions::default())).await?;
    println!("   💾 Wrote temp file: {}", temp_file);
    
    // Move to final location
    fs.move_file(temp_file, final_file).await?;
    println!("   🔄 Moved to final file: {}", final_file);
    
    // Verify final file
    let final_exists = fs.exists(final_file).await?;
    let temp_exists = fs.exists(temp_file).await?;
    println!("   📄 Final file exists: {}", final_exists);
    println!("   📄 Temp file exists: {}", temp_exists);
    
    // Final check: List all files recursively
    println!("\n7️⃣ Final file listing...");
    count_and_list_files_recursive(temp_path)?;
    
    println!("\n✅ All filesystem tests completed successfully!");
    Ok(())
}

fn count_and_list_files_recursive(dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut count = 0;
    
    fn list_recursive(dir: &std::path::Path, level: usize, count: &mut usize) -> Result<(), Box<dyn std::error::Error>> {
        if !dir.exists() {
            return Ok(());
        }
        
        let indent = "  ".repeat(level);
        
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy();
            
            if path.is_dir() {
                println!("{}📁 {}/", indent, name);
                list_recursive(&path, level + 1, count)?;
            } else {
                let size = entry.metadata()?.len();
                println!("{}📄 {} ({} bytes)", indent, name, size);
                *count += 1;
            }
        }
        Ok(())
    }
    
    println!("📁 Full directory tree:");
    list_recursive(dir, 0, &mut count)?;
    println!("📊 Total files: {}", count);
    
    Ok(())
}
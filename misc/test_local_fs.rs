use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Creating test directory...");
    std::fs::create_dir_all("/tmp/proximadb-fs-tests")?;
    
    println!("Creating filesystem config...");
    let mut config = FilesystemConfig::default();
    config.local = Some(proximadb::storage::persistence::filesystem::local::LocalConfig::default());
    
    println!("Creating filesystem factory...");
    let factory = FilesystemFactory::new(config).await?;
    
    println!("Getting filesystem for test path...");
    let fs = factory.get_filesystem("file:///tmp/proximadb-fs-tests/test.txt")?;
    
    println!("Writing test data...");
    let test_data = b"Hello from test!";
    fs.write("/tmp/proximadb-fs-tests/test.txt", test_data, None).await?;
    
    println!("Reading test data...");
    let read_data = fs.read("/tmp/proximadb-fs-tests/test.txt").await?;
    
    assert_eq!(test_data, &read_data[..]);
    println!("Test passed!");
    
    Ok(())
}
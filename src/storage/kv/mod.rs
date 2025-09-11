use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait StorageKV: Send + Sync {
    async fn put(&self, key: &str, value: &[u8]) -> Result<()>;
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    async fn delete(&self, key: &str) -> Result<()>;
}

pub struct FsKV {
    base_dir: std::path::PathBuf,
}

impl FsKV {
    pub fn new<P: Into<std::path::PathBuf>>(base_dir: P) -> Self {
        Self { base_dir: base_dir.into() }
    }
    fn path_for(&self, key: &str) -> std::path::PathBuf {
        let mut p = self.base_dir.clone();
        p.push(format!("{}.bin", key.replace('/', "__")));
        p
    }
}

#[async_trait]
impl StorageKV for FsKV {
    async fn put(&self, key: &str, value: &[u8]) -> Result<()> {
        let path = self.path_for(key);
        if let Some(parent) = path.parent() { tokio::fs::create_dir_all(parent).await.ok(); }
        let mut f = tokio::fs::File::create(&path).await?;
        use tokio::io::AsyncWriteExt;
        f.write_all(value).await?;
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path_for(key);
        match tokio::fs::read(&path).await { Ok(b) => Ok(Some(b)), Err(_) => Ok(None) }
    }
    async fn delete(&self, key: &str) -> Result<()> {
        let path = self.path_for(key);
        let _ = tokio::fs::remove_file(&path).await;
        Ok(())
    }
}


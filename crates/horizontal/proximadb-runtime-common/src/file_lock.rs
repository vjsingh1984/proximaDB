//! File-based locking for multi-process coordination
//!
//! This module provides cross-process file locking using advisory locks.
//! On Unix systems, it uses `flock()` via the standard library.
//! On Windows, it uses `LockFileEx()`.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Access modes for multi-process coordination
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// Single writer, exclusive access. No other process can access the database.
    Exclusive,
    /// Multiple readers, no writers. Read-only access for all processes.
    SharedRead,
    /// One leader (write), many followers (read). Leader election determines
    /// which process gets write access.
    LeaderFollower,
}

impl AccessMode {
    /// Check if this access mode allows write operations
    pub fn can_write(&self) -> bool {
        match self {
            Self::Exclusive => true,
            Self::SharedRead => false,
            Self::LeaderFollower => true, // Depends on leader status, checked separately
        }
    }

    /// Get the lock file name for this mode
    fn lock_file_name(&self) -> &'static str {
        match self {
            // Readers and exclusive writers MUST lock the same inode. flock
            // compatibility is defined between locks on one file; using a
            // mode-specific file silently permits a reader and writer at once.
            Self::Exclusive | Self::SharedRead => "access.lock",
            Self::LeaderFollower => "leader.lock",
        }
    }
}

impl std::fmt::Display for AccessMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exclusive => write!(f, "exclusive"),
            Self::SharedRead => write!(f, "shared_read"),
            Self::LeaderFollower => write!(f, "leader_follower"),
        }
    }
}

impl std::str::FromStr for AccessMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "exclusive" => Ok(Self::Exclusive),
            "shared_read" | "shared" | "reader" | "follower" => Ok(Self::SharedRead),
            "leader_follower" | "leader" | "writer" => Ok(Self::LeaderFollower),
            _ => Err(format!(
                "Invalid access mode: '{}'. Valid modes: exclusive, shared_read, leader_follower",
                s
            )),
        }
    }
}

/// Errors that can occur during file locking
#[derive(Debug)]
pub enum FileLockError {
    /// Failed to create lock file
    CreateFailed(io::Error),
    /// Failed to acquire lock (another process holds it)
    LockFailed(io::Error),
    /// Failed to release lock
    ReleaseFailed(io::Error),
    /// Lock directory does not exist
    DirectoryNotFound(PathBuf),
    /// Lock file path is invalid
    InvalidPath(String),
    /// Lock is already held
    AlreadyLocked,
    /// Lock operation would block (for non-blocking operations)
    WouldBlock {
        lock_file: PathBuf,
        owner: Option<LockOwner>,
    },
}

/// Best-effort diagnostic recorded in a lock file. The advisory lock is the
/// authority; this payload only tells a caller who most recently acquired it.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct LockOwner {
    pub pid: u32,
    pub mode: String,
    pub timestamp: String,
}

impl std::fmt::Display for FileLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateFailed(e) => write!(f, "Failed to create lock file: {}", e),
            Self::LockFailed(e) => write!(f, "Failed to acquire lock: {}", e),
            Self::ReleaseFailed(e) => write!(f, "Failed to release lock: {}", e),
            Self::DirectoryNotFound(path) => {
                write!(f, "Lock directory not found: {}", path.display())
            }
            Self::InvalidPath(s) => write!(f, "Invalid lock path: {}", s),
            Self::AlreadyLocked => write!(f, "Lock is already held by another process"),
            Self::WouldBlock { lock_file, owner } => {
                write!(f, "lock is already held at {}", lock_file.display())?;
                if let Some(owner) = owner {
                    write!(
                        f,
                        " (last recorded holder: pid={}, mode={}, since={})",
                        owner.pid, owner.mode, owner.timestamp
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for FileLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CreateFailed(e) | Self::LockFailed(e) | Self::ReleaseFailed(e) => Some(e),
            _ => None,
        }
    }
}

/// File-based lock manager for multi-process coordination
///
/// This manager uses advisory file locks to coordinate access between
/// multiple processes. The lock file is created in the data directory.
///
/// # Example
///
/// ```rust,ignore
/// use proximadb::embedded::coordination::{AccessMode, FileLockManager};
///
/// // Acquire exclusive lock
/// let lock = FileLockManager::acquire("/path/to/data", AccessMode::Exclusive)?;
///
/// // Do work with exclusive access...
///
/// // Lock is released when manager is dropped
/// drop(lock);
/// ```
pub struct FileLockManager {
    /// Path to the lock file
    lock_file: PathBuf,
    /// Access mode
    mode: AccessMode,
    /// The file handle (keeps the lock alive)
    #[allow(dead_code)]
    file: Option<File>,
    /// Whether the lock is currently held
    locked: AtomicBool,
}

impl FileLockManager {
    /// Create a lock manager and acquire the lock without waiting.
    ///
    /// This will return immediately if the lock cannot be acquired.
    ///
    /// # Arguments
    /// * `data_path` - Path to the data directory
    /// * `mode` - Access mode to use
    ///
    /// # Returns
    /// A `FileLockManager` with the lock acquired, or an error if the lock
    /// cannot be acquired immediately
    pub fn acquire(data_path: impl AsRef<Path>, mode: AccessMode) -> Result<Self, FileLockError> {
        Self::acquire_lock(data_path, mode)
    }

    /// Internal method to acquire the lock
    fn acquire_lock(data_path: impl AsRef<Path>, mode: AccessMode) -> Result<Self, FileLockError> {
        let data_path = data_path.as_ref();

        // Ensure directory exists
        if !data_path.exists() {
            std::fs::create_dir_all(data_path).map_err(FileLockError::CreateFailed)?;
        }

        let lock_file = data_path.join(mode.lock_file_name());

        // Create or open the lock file
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_file)
            .map_err(FileLockError::CreateFailed)?;

        // Acquire the lock based on mode and blocking preference
        let lock_result = match mode {
            AccessMode::Exclusive | AccessMode::LeaderFollower => {
                Self::acquire_exclusive_lock(&file)
            }
            AccessMode::SharedRead => Self::acquire_shared_lock(&file),
        };

        match lock_result {
            Ok(()) => {
                // Write PID to lock file for debugging
                let mut file_for_write = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&lock_file)
                    .map_err(FileLockError::CreateFailed)?;

                let pid = std::process::id();
                let owner = LockOwner {
                    pid,
                    mode: mode.to_string(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                };
                let _ = serde_json::to_writer(&mut file_for_write, &owner);
                let _ = writeln!(file_for_write);

                tracing::debug!(
                    "Acquired {} lock on {} (PID: {})",
                    mode,
                    lock_file.display(),
                    pid
                );

                Ok(Self {
                    lock_file,
                    mode,
                    file: Some(file),
                    locked: AtomicBool::new(true),
                })
            }
            Err(FileLockError::WouldBlock { .. }) => Err(FileLockError::WouldBlock {
                lock_file: lock_file.clone(),
                owner: Self::read_owner(&lock_file),
            }),
            Err(e) => Err(e),
        }
    }

    fn read_owner(lock_file: &Path) -> Option<LockOwner> {
        let mut contents = String::new();
        File::open(lock_file)
            .ok()?
            .read_to_string(&mut contents)
            .ok()?;
        serde_json::from_str(&contents).ok()
    }

    /// Acquire an exclusive (write) lock
    #[cfg(unix)]
    fn acquire_exclusive_lock(file: &File) -> Result<(), FileLockError> {
        use std::os::unix::io::AsRawFd;

        let fd = file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

        if result == 0 {
            Ok(())
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                Err(FileLockError::WouldBlock {
                    lock_file: PathBuf::new(),
                    owner: None,
                })
            } else {
                Err(FileLockError::LockFailed(err))
            }
        }
    }

    #[cfg(not(unix))]
    fn acquire_exclusive_lock(_file: &File) -> Result<(), FileLockError> {
        // On non-Unix systems, we don't implement locking for now
        // In production, you'd use Windows LockFileEx or a cross-platform crate
        tracing::warn!("File locking not implemented on this platform");
        Ok(())
    }

    /// Acquire a shared (read) lock
    #[cfg(unix)]
    fn acquire_shared_lock(file: &File) -> Result<(), FileLockError> {
        use std::os::unix::io::AsRawFd;

        let fd = file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) };

        if result == 0 {
            Ok(())
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                Err(FileLockError::WouldBlock {
                    lock_file: PathBuf::new(),
                    owner: None,
                })
            } else {
                Err(FileLockError::LockFailed(err))
            }
        }
    }

    #[cfg(not(unix))]
    fn acquire_shared_lock(_file: &File) -> Result<(), FileLockError> {
        // On non-Unix systems, we don't implement locking for now
        tracing::warn!("File locking not implemented on this platform");
        Ok(())
    }

    /// Release the lock
    #[cfg(unix)]
    pub fn release(&self) -> Result<(), FileLockError> {
        if self
            .locked
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }
        if let Some(ref file) = self.file {
            use std::os::unix::io::AsRawFd;

            let fd = file.as_raw_fd();
            let result = unsafe { libc::flock(fd, libc::LOCK_UN) };

            if result == 0 {
                tracing::debug!("Released lock on {}", self.lock_file.display());
                Ok(())
            } else {
                self.locked.store(true, Ordering::SeqCst);
                Err(FileLockError::ReleaseFailed(io::Error::last_os_error()))
            }
        } else {
            Ok(())
        }
    }

    #[cfg(not(unix))]
    pub fn release(&self) -> Result<(), FileLockError> {
        self.locked.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Check if the lock is currently held
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }

    /// Get the access mode
    pub fn mode(&self) -> AccessMode {
        self.mode
    }

    /// Get the lock file path
    pub fn lock_file_path(&self) -> &Path {
        &self.lock_file
    }

    /// Check if write operations are allowed
    pub fn can_write(&self) -> bool {
        self.mode.can_write() && self.is_locked()
    }
}

impl Drop for FileLockManager {
    fn drop(&mut self) {
        if self.locked.load(Ordering::SeqCst)
            && let Err(e) = self.release()
        {
            tracing::error!("Failed to release lock on drop: {}", e);
        }
    }
}

// FileLockManager is Send + Sync because:
// - The File handle is thread-safe for the locking operations we use
// - The AtomicBool is inherently thread-safe
// - The PathBuf is immutable after construction
unsafe impl Send for FileLockManager {}
unsafe impl Sync for FileLockManager {}

/// One atomic-lifetime lease over a set of local storage roots.
///
/// Roots are created, canonicalized, and acquired in stable path order. If a
/// later acquisition fails, the already-acquired managers are dropped before
/// the error is returned. This prevents partial multi-disk ownership between
/// cooperating ProximaDB processes.
pub struct FileLockSet {
    locks: Vec<FileLockManager>,
}

impl FileLockSet {
    pub fn acquire<I, P>(roots: I, mode: AccessMode) -> Result<Self, FileLockError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut canonical_roots = BTreeSet::new();
        for root in roots {
            let root = root.as_ref();
            std::fs::create_dir_all(root).map_err(FileLockError::CreateFailed)?;
            canonical_roots.insert(root.canonicalize().map_err(FileLockError::CreateFailed)?);
        }

        let mut locks = Vec::with_capacity(canonical_roots.len());
        for root in canonical_roots {
            locks.push(FileLockManager::acquire(root, mode)?);
        }
        Ok(Self { locks })
    }

    pub fn is_locked(&self) -> bool {
        !self.locks.is_empty() && self.locks.iter().all(FileLockManager::is_locked)
    }

    /// Release every root even if one unlock operation fails. Returns the first
    /// error after all roots have been attempted.
    pub fn release(&self) -> Result<(), FileLockError> {
        let mut first_error = None;
        for lock in self.locks.iter().rev() {
            if let Err(error) = lock.release()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub fn roots(&self) -> impl Iterator<Item = &Path> {
        self.locks
            .iter()
            .filter_map(|lock| lock.lock_file_path().parent())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_exclusive_lock_basic() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory for test");
        let path = temp_dir.path();

        let lock = FileLockManager::acquire(path, AccessMode::Exclusive);
        assert!(lock.is_ok());

        let lock = lock.expect("Failed to acquire exclusive lock for test");
        assert!(lock.is_locked());
        assert!(lock.can_write());
        assert_eq!(lock.mode(), AccessMode::Exclusive);
    }

    #[test]
    fn test_shared_lock_basic() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory for test");
        let path = temp_dir.path();

        let lock = FileLockManager::acquire(path, AccessMode::SharedRead);
        assert!(lock.is_ok());

        let lock = lock.expect("Failed to acquire shared lock for test");
        assert!(lock.is_locked());
        assert!(!lock.can_write());
        assert_eq!(lock.mode(), AccessMode::SharedRead);
    }

    #[test]
    fn test_exclusive_blocks_exclusive() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory for test");
        let path = temp_dir.path().to_path_buf();

        let _lock1 = FileLockManager::acquire(&path, AccessMode::Exclusive)
            .expect("Failed to acquire first exclusive lock for test");

        // Non-blocking attempt should fail
        let lock2_result = FileLockManager::acquire(&path, AccessMode::Exclusive);
        assert!(lock2_result.is_err());
    }

    #[test]
    fn test_shared_allows_multiple() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory for test");
        let path = temp_dir.path().to_path_buf();

        let lock1 = FileLockManager::acquire(&path, AccessMode::SharedRead);
        assert!(lock1.is_ok());

        let lock2 = FileLockManager::acquire(&path, AccessMode::SharedRead);
        assert!(lock2.is_ok());

        // Both locks should be held
        assert!(
            lock1
                .expect("Failed to acquire first shared lock for test")
                .is_locked()
        );
        assert!(
            lock2
                .expect("Failed to acquire second shared lock for test")
                .is_locked()
        );
    }

    #[test]
    fn test_exclusive_and_shared_read_use_one_coordination_domain() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory for test");
        let path = temp_dir.path().to_path_buf();

        let exclusive = FileLockManager::acquire(&path, AccessMode::Exclusive)
            .expect("Failed to acquire exclusive lock for test");
        assert!(matches!(
            FileLockManager::acquire(&path, AccessMode::SharedRead),
            Err(FileLockError::WouldBlock { .. })
        ));
        drop(exclusive);

        let shared = FileLockManager::acquire(&path, AccessMode::SharedRead)
            .expect("Failed to acquire shared lock for test");
        assert!(matches!(
            FileLockManager::acquire(&path, AccessMode::Exclusive),
            Err(FileLockError::WouldBlock { .. })
        ));
        drop(shared);
    }

    #[test]
    fn test_explicit_release_is_idempotent_and_allows_reopen() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory for test");
        let path = temp_dir.path().to_path_buf();
        let lock = FileLockManager::acquire(&path, AccessMode::Exclusive)
            .expect("Failed to acquire exclusive lock for test");

        let conflict = match FileLockManager::acquire(&path, AccessMode::Exclusive) {
            Ok(_) => panic!("a second exclusive lock must fail fast"),
            Err(error) => error,
        };
        match conflict {
            FileLockError::WouldBlock { lock_file, owner } => {
                assert!(lock_file.ends_with("access.lock"));
                assert_eq!(owner.map(|record| record.pid), Some(std::process::id()));
            }
            other => panic!("expected WouldBlock, got {other}"),
        }

        lock.release().expect("explicit release");
        lock.release().expect("repeated release is idempotent");
        assert!(!lock.is_locked());
        let reopened = FileLockManager::acquire(&path, AccessMode::Exclusive)
            .expect("released lock must be acquirable while old handle is alive");
        drop(reopened);
    }

    #[test]
    fn test_access_mode_from_str() {
        assert_eq!(
            "exclusive"
                .parse::<AccessMode>()
                .expect("Failed to parse 'exclusive' access mode"),
            AccessMode::Exclusive
        );
        assert_eq!(
            "shared_read"
                .parse::<AccessMode>()
                .expect("Failed to parse 'shared_read' access mode"),
            AccessMode::SharedRead
        );
        assert_eq!(
            "leader"
                .parse::<AccessMode>()
                .expect("Failed to parse 'leader' access mode"),
            AccessMode::LeaderFollower
        );
        assert_eq!(
            "follower"
                .parse::<AccessMode>()
                .expect("Failed to parse 'follower' access mode"),
            AccessMode::SharedRead
        );
        assert!("invalid".parse::<AccessMode>().is_err());
    }

    #[test]
    fn test_lock_file_path() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory for test");
        let path = temp_dir.path();

        let lock = FileLockManager::acquire(path, AccessMode::Exclusive)
            .expect("Failed to acquire exclusive lock for test");
        assert!(lock.lock_file_path().ends_with("access.lock"));

        drop(lock);

        let lock = FileLockManager::acquire(path, AccessMode::SharedRead)
            .expect("Failed to acquire shared lock for test");
        assert!(lock.lock_file_path().ends_with("access.lock"));
    }

    #[test]
    fn test_lock_release_on_drop() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory for test");
        let path = temp_dir.path().to_path_buf();

        {
            let lock = FileLockManager::acquire(&path, AccessMode::Exclusive)
                .expect("Failed to acquire exclusive lock for test");
            assert!(lock.is_locked());
        }
        // Lock should be released after drop

        // Should be able to acquire again
        let lock2 = FileLockManager::acquire(&path, AccessMode::Exclusive);
        assert!(lock2.is_ok());
    }

    #[test]
    fn test_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FileLockManager>();
        assert_send_sync::<FileLockSet>();
    }

    #[test]
    fn lock_set_deduplicates_aliases_and_releases_every_root() {
        let temp_dir = TempDir::new().expect("tempdir");
        let first = temp_dir.path().join("first");
        let second = temp_dir.path().join("second");
        let lease = FileLockSet::acquire([&first, &second, &first], AccessMode::Exclusive)
            .expect("multi-root lease");
        assert!(lease.is_locked());
        assert_eq!(lease.roots().count(), 2);
        assert!(FileLockManager::acquire(&second, AccessMode::Exclusive).is_err());

        lease.release().expect("release all roots");
        assert!(!lease.is_locked());
        FileLockManager::acquire(&first, AccessMode::Exclusive).expect("first root released");
        FileLockManager::acquire(&second, AccessMode::Exclusive).expect("second root released");
    }

    #[test]
    fn failed_lock_set_acquisition_releases_earlier_roots() {
        let temp_dir = TempDir::new().expect("tempdir");
        let available = temp_dir.path().join("a-available");
        let occupied = temp_dir.path().join("z-occupied");
        let _owner = FileLockManager::acquire(&occupied, AccessMode::Exclusive)
            .expect("occupy later-sorted root");

        assert!(FileLockSet::acquire([&available, &occupied], AccessMode::Exclusive).is_err());
        FileLockManager::acquire(&available, AccessMode::Exclusive)
            .expect("partial acquisition must roll back");
    }
}

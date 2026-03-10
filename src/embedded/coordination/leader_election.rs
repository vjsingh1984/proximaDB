//! Leader election for multi-process coordination
//!
//! This module provides leader election using file-based locking.
//! Only one process can be the leader at a time, and the leader
//! has write access while followers have read-only access.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use super::file_lock::{AccessMode, FileLockManager};

/// Leader election status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaderStatus {
    /// This node is the leader
    Leader,
    /// This node is a follower
    Follower,
    /// Status is unknown (election in progress)
    Unknown,
}

impl LeaderStatus {
    /// Check if this status allows write operations
    pub fn can_write(&self) -> bool {
        matches!(self, Self::Leader)
    }
}

/// Errors that can occur during leader election
#[derive(Debug)]
pub enum LeaderElectionError {
    /// Failed to acquire leader lock
    LockFailed(io::Error),
    /// Failed to read leader info
    ReadFailed(io::Error),
    /// Failed to write leader info
    WriteFailed(io::Error),
    /// Invalid leader info format
    InvalidFormat(String),
    /// Election directory not found
    DirectoryNotFound(PathBuf),
    /// Node ID is invalid
    InvalidNodeId(String),
}

impl std::fmt::Display for LeaderElectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LockFailed(e) => write!(f, "Failed to acquire leader lock: {}", e),
            Self::ReadFailed(e) => write!(f, "Failed to read leader info: {}", e),
            Self::WriteFailed(e) => write!(f, "Failed to write leader info: {}", e),
            Self::InvalidFormat(s) => write!(f, "Invalid leader info format: {}", s),
            Self::DirectoryNotFound(path) => {
                write!(f, "Election directory not found: {}", path.display())
            }
            Self::InvalidNodeId(s) => write!(f, "Invalid node ID: {}", s),
        }
    }
}

impl std::error::Error for LeaderElectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LockFailed(e) | Self::ReadFailed(e) | Self::WriteFailed(e) => Some(e),
            _ => None,
        }
    }
}

/// Leader info stored in the election file
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LeaderInfo {
    /// Node ID of the leader
    node_id: String,
    /// Process ID
    pid: u32,
    /// Timestamp when leadership was acquired
    timestamp: String,
    /// Hostname of the leader
    hostname: String,
}

/// Leader election manager
///
/// This uses file-based locking to elect a leader among multiple processes.
/// The process that successfully acquires the exclusive lock becomes the leader.
///
/// # Example
///
/// ```rust,ignore
/// use proximadb::embedded::coordination::LeaderElection;
///
/// // Try to become leader
/// let election = LeaderElection::new("/path/to/data", "node1")?;
///
/// if election.is_leader() {
///     // This process is the leader, can write
///     println!("I am the leader!");
/// } else {
///     // This process is a follower, read-only
///     println!("Following leader: {:?}", election.leader_id());
/// }
/// ```
pub struct LeaderElection {
    /// Node ID for this process
    node_id: String,
    /// File lock manager for leader lock
    lock_manager: Option<FileLockManager>,
    /// Path to the leader info file
    leader_info_path: PathBuf,
    /// Whether this node is currently the leader
    is_leader: AtomicBool,
    /// Current known leader ID (None if unknown)
    leader_id: RwLock<Option<String>>,
}

impl LeaderElection {
    const LEADER_INFO_FILE: &'static str = "leader_info.json";

    /// Create a new leader election and attempt to become leader
    ///
    /// This will try to acquire the leader lock. If successful, this node
    /// becomes the leader. If not, it becomes a follower and reads the
    /// current leader info.
    ///
    /// # Arguments
    /// * `data_path` - Path to the data directory
    /// * `node_id` - Unique identifier for this node
    ///
    /// # Returns
    /// A `LeaderElection` instance
    pub fn new(
        data_path: impl AsRef<Path>,
        node_id: impl Into<String>,
    ) -> Result<Self, LeaderElectionError> {
        let data_path = data_path.as_ref();
        let node_id = node_id.into();

        if node_id.is_empty() {
            return Err(LeaderElectionError::InvalidNodeId(
                "Node ID cannot be empty".to_string(),
            ));
        }

        // Ensure directory exists
        if !data_path.exists() {
            std::fs::create_dir_all(data_path)
                .map_err(|_e| LeaderElectionError::DirectoryNotFound(data_path.to_path_buf()))?;
        }

        let leader_info_path = data_path.join(Self::LEADER_INFO_FILE);

        // Try to acquire leader lock (non-blocking)
        let lock_result = FileLockManager::try_new(data_path, AccessMode::LeaderFollower);

        match lock_result {
            Ok(lock_manager) => {
                // We got the lock, we are the leader
                let election = Self {
                    node_id: node_id.clone(),
                    lock_manager: Some(lock_manager),
                    leader_info_path,
                    is_leader: AtomicBool::new(true),
                    leader_id: RwLock::new(Some(node_id.clone())),
                };

                // Write leader info
                election.write_leader_info()?;

                tracing::info!(
                    "Leader election: Node '{}' became leader (PID: {})",
                    node_id,
                    std::process::id()
                );

                Ok(election)
            }
            Err(_) => {
                // Failed to get lock, we are a follower
                // Try to read current leader info
                let current_leader = Self::read_leader_info_from_file(&leader_info_path);

                tracing::info!(
                    "Leader election: Node '{}' became follower (leader: {:?})",
                    node_id,
                    current_leader
                );

                Ok(Self {
                    node_id,
                    lock_manager: None,
                    leader_info_path,
                    is_leader: AtomicBool::new(false),
                    leader_id: RwLock::new(current_leader),
                })
            }
        }
    }

    /// Check if this node is currently the leader
    pub fn is_leader(&self) -> bool {
        self.is_leader.load(Ordering::SeqCst)
    }

    /// Get the current leader status
    pub fn status(&self) -> LeaderStatus {
        if self.is_leader() {
            LeaderStatus::Leader
        } else {
            LeaderStatus::Follower
        }
    }

    /// Get the current leader ID (if known)
    pub fn leader_id(&self) -> Option<String> {
        self.leader_id.read().ok().and_then(|guard| guard.clone())
    }

    /// Get this node's ID
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Check if write operations are allowed
    pub fn can_write(&self) -> bool {
        self.is_leader()
    }

    /// Refresh the leader info (re-read from file)
    ///
    /// This is useful for followers to check if the leader has changed.
    pub fn refresh_leader_info(&self) -> Result<(), LeaderElectionError> {
        if !self.is_leader() {
            let leader_id = Self::read_leader_info_from_file(&self.leader_info_path);
            if let Ok(mut guard) = self.leader_id.write() {
                *guard = leader_id;
            }
        }
        Ok(())
    }

    /// Write leader info to file
    fn write_leader_info(&self) -> Result<(), LeaderElectionError> {
        let info = LeaderInfo {
            node_id: self.node_id.clone(),
            pid: std::process::id(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            hostname: hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string()),
        };

        let json = serde_json::to_string_pretty(&info)
            .map_err(|e| LeaderElectionError::InvalidFormat(e.to_string()))?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.leader_info_path)
            .map_err(LeaderElectionError::WriteFailed)?;

        file.write_all(json.as_bytes())
            .map_err(LeaderElectionError::WriteFailed)?;

        file.sync_all().map_err(LeaderElectionError::WriteFailed)?;

        Ok(())
    }

    /// Read leader info from file
    fn read_leader_info_from_file(path: &Path) -> Option<String> {
        if !path.exists() {
            return None;
        }

        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return None,
        };

        let mut contents = String::new();
        if file.read_to_string(&mut contents).is_err() {
            return None;
        }

        match serde_json::from_str::<LeaderInfo>(&contents) {
            Ok(info) => Some(info.node_id),
            Err(_) => None,
        }
    }

    /// Step down from leadership (if currently leader)
    ///
    /// This releases the leader lock and allows another node to become leader.
    pub fn step_down(&mut self) -> Result<(), LeaderElectionError> {
        if self.is_leader() {
            self.is_leader.store(false, Ordering::SeqCst);
            self.lock_manager = None;

            if let Ok(mut guard) = self.leader_id.write() {
                *guard = None;
            }

            tracing::info!("Leader election: Node '{}' stepped down", self.node_id);
        }
        Ok(())
    }

    /// Try to become leader (if currently a follower)
    ///
    /// This attempts to acquire the leader lock. If successful, this node
    /// becomes the leader.
    ///
    /// # Returns
    /// `true` if this node became the leader, `false` otherwise
    pub fn try_become_leader(
        &mut self,
        data_path: impl AsRef<Path>,
    ) -> Result<bool, LeaderElectionError> {
        if self.is_leader() {
            return Ok(true);
        }

        let lock_result = FileLockManager::try_new(data_path, AccessMode::LeaderFollower);

        match lock_result {
            Ok(lock_manager) => {
                self.lock_manager = Some(lock_manager);
                self.is_leader.store(true, Ordering::SeqCst);

                if let Ok(mut guard) = self.leader_id.write() {
                    *guard = Some(self.node_id.clone());
                }

                self.write_leader_info()?;

                tracing::info!("Leader election: Node '{}' became leader", self.node_id);

                Ok(true)
            }
            Err(_) => {
                // Update leader info
                let _ = self.refresh_leader_info();
                Ok(false)
            }
        }
    }
}

impl Drop for LeaderElection {
    fn drop(&mut self) {
        if self.is_leader() {
            tracing::debug!(
                "Leader election: Node '{}' dropping leadership",
                self.node_id
            );
        }
        // Lock is released automatically when lock_manager is dropped
    }
}

// LeaderElection is Send + Sync because all its fields are thread-safe
unsafe impl Send for LeaderElection {}
unsafe impl Sync for LeaderElection {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_leader_election_first_becomes_leader() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let path = temp_dir.path();

        let election =
            LeaderElection::new(path, "node1").expect("Failed to create leader election");
        assert!(election.is_leader());
        assert_eq!(election.leader_id(), Some("node1".to_string()));
        assert_eq!(election.status(), LeaderStatus::Leader);
        assert!(election.can_write());
    }

    #[test]
    fn test_leader_election_second_becomes_follower() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let path = temp_dir.path().to_path_buf();

        let election1 = LeaderElection::new(&path, "node1")
            .expect("Failed to create leader election for node1");
        assert!(election1.is_leader());

        let election2 = LeaderElection::new(&path, "node2")
            .expect("Failed to create leader election for node2");
        assert!(!election2.is_leader());
        assert_eq!(election2.status(), LeaderStatus::Follower);
        assert!(!election2.can_write());

        // Follower should know the leader
        assert_eq!(election2.leader_id(), Some("node1".to_string()));
    }

    #[test]
    fn test_leader_step_down() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let path = temp_dir.path().to_path_buf();

        let mut election1 = LeaderElection::new(&path, "node1")
            .expect("Failed to create leader election for node1");
        assert!(election1.is_leader());

        election1
            .step_down()
            .expect("Failed to step down from leadership");
        assert!(!election1.is_leader());

        // Now another node can become leader
        let election2 = LeaderElection::new(&path, "node2")
            .expect("Failed to create leader election for node2");
        assert!(election2.is_leader());
    }

    #[test]
    fn test_leader_info_file_created() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let path = temp_dir.path();

        let _election =
            LeaderElection::new(path, "test_node").expect("Failed to create leader election");

        let info_path = path.join(LeaderElection::LEADER_INFO_FILE);
        assert!(info_path.exists());

        let contents =
            std::fs::read_to_string(&info_path).expect("Failed to read leader info file");
        let info: LeaderInfo =
            serde_json::from_str(&contents).expect("Failed to parse leader info JSON");
        assert_eq!(info.node_id, "test_node");
        assert_eq!(info.pid, std::process::id());
    }

    #[test]
    fn test_invalid_node_id() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let path = temp_dir.path();

        let result = LeaderElection::new(path, "");
        assert!(result.is_err());
    }

    #[test]
    fn test_refresh_leader_info() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let path = temp_dir.path().to_path_buf();

        let election1 = LeaderElection::new(&path, "node1")
            .expect("Failed to create leader election for node1");
        let election2 = LeaderElection::new(&path, "node2")
            .expect("Failed to create leader election for node2");

        assert!(election1.is_leader());
        assert!(!election2.is_leader());

        // Refresh should work without error
        election2
            .refresh_leader_info()
            .expect("Failed to refresh leader info");
        assert_eq!(election2.leader_id(), Some("node1".to_string()));
    }

    #[test]
    fn test_try_become_leader() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let path = temp_dir.path().to_path_buf();

        let mut election1 = LeaderElection::new(&path, "node1")
            .expect("Failed to create leader election for node1");
        let mut election2 = LeaderElection::new(&path, "node2")
            .expect("Failed to create leader election for node2");

        assert!(election1.is_leader());
        assert!(!election2.is_leader());

        // Node2 tries to become leader while node1 is still leader
        let became_leader = election2
            .try_become_leader(&path)
            .expect("Failed to try becoming leader");
        assert!(!became_leader);

        // Node1 steps down
        election1
            .step_down()
            .expect("Failed to step down from leadership");

        // Now node2 can become leader
        let became_leader = election2
            .try_become_leader(&path)
            .expect("Failed to try becoming leader");
        assert!(became_leader);
        assert!(election2.is_leader());
    }

    #[test]
    fn test_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LeaderElection>();
    }
}

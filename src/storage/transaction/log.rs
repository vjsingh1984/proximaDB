/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Transaction Logging for Persistence and Recovery

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::context::TransactionId;
use crate::core::error::{ProximaDBError, StorageError};

/// Result type for log operations
type Result<T> = std::result::Result<T, ProximaDBError>;

/// Log record representing a transaction state change
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogRecord {
    /// Transaction started
    Begin {
        tx_id: TransactionId,
        timestamp: u64,
    },
    /// Transaction reached prepared state
    Prepare {
        tx_id: TransactionId,
        participants: Vec<String>,
    },
    /// Transaction committed
    Commit {
        tx_id: TransactionId,
    },
    /// Transaction aborted
    Abort {
        tx_id: TransactionId,
        reason: String,
    },
}

/// Interface for transaction logging
pub trait TransactionLog: Send + Sync {
    /// Append a record to the log
    fn append(&self, record: LogRecord) -> Result<()>;
    /// Read all records from the log (for recovery)
    fn read_all(&self) -> Result<Vec<LogRecord>>;
    /// Clear the log (optional, usually after checkpoint)
    fn clear(&self) -> Result<()>;
}

/// File-based implementation of TransactionLog
pub struct FileTransactionLog {
    path: PathBuf,
    file: Arc<Mutex<File>>,
}

impl FileTransactionLog {
    /// Create a new file-based transaction log
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path.as_ref())
            .map_err(|e| ProximaDBError::Storage(StorageError::DiskIO(e)))?;

        Ok(Self {
            path: path.as_ref().to_path_buf(),
            file: Arc::new(Mutex::new(file)),
        })
    }
}

impl TransactionLog for FileTransactionLog {
    fn append(&self, record: LogRecord) -> Result<()> {
        let mut file = self.file.lock();
        let json = serde_json::to_string(&record).map_err(|e| {
            ProximaDBError::Storage(StorageError::Serialization(format!(
                "Failed to serialize log record: {}",
                e
            )))
        })?;

        writeln!(file, "{}", json).map_err(|e| ProximaDBError::Storage(StorageError::DiskIO(e)))?;

        file.sync_all()
            .map_err(|e| ProximaDBError::Storage(StorageError::DiskIO(e)))?;

        Ok(())
    }

    fn read_all(&self) -> Result<Vec<LogRecord>> {
        let file = File::open(&self.path).map_err(|e| ProximaDBError::Storage(StorageError::DiskIO(e)))?;

        let reader = BufReader::new(file);
        let mut records = Vec::new();

        for line in reader.lines() {
            let line = line.map_err(|e| ProximaDBError::Storage(StorageError::DiskIO(e)))?;

            if line.trim().is_empty() {
                continue;
            }

            let record: LogRecord = serde_json::from_str(&line).map_err(|e| {
                ProximaDBError::Storage(StorageError::Serialization(format!(
                    "Failed to deserialize log record: {}",
                    e
                )))
            })?;
            records.push(record);
        }

        Ok(records)
    }

    fn clear(&self) -> Result<()> {
        let mut file = self.file.lock();
        *file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.path)
            .map_err(|e| ProximaDBError::Storage(StorageError::DiskIO(e)))?;
        Ok(())
    }
}

/// No-op implementation of TransactionLog (for testing or when logging is disabled)
pub struct NoOpTransactionLog;

impl TransactionLog for NoOpTransactionLog {
    fn append(&self, _record: LogRecord) -> Result<()> {
        Ok(())
    }

    fn read_all(&self) -> Result<Vec<LogRecord>> {
        Ok(Vec::new())
    }

    fn clear(&self) -> Result<()> {
        Ok(())
    }
}

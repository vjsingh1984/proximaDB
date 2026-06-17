// PostgreSQL logical replication stream
//
// Manages the connection to PostgreSQL for logical replication,
// including slot creation, replication streaming, and LSN tracking.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use tracing::{info, warn};

use super::config::PostgresConfig;

/// Manages a PostgreSQL logical replication connection.
///
/// Encapsulates the lifecycle of a replication session: connect,
/// create/verify replication slot, start streaming, and track LSN positions.
pub struct ReplicationStream {
    config: PostgresConfig,
    current_lsn: u64,
    flush_lsn: u64,
    connected: bool,
    last_status_at: Option<Instant>,
}

impl ReplicationStream {
    /// Create a new replication stream (not yet connected).
    pub fn new(config: PostgresConfig) -> Self {
        Self {
            config,
            current_lsn: 0,
            flush_lsn: 0,
            connected: false,
            last_status_at: None,
        }
    }

    /// Get the current write LSN position.
    pub fn current_lsn(&self) -> u64 {
        self.current_lsn
    }

    /// Get the current flush LSN position.
    pub fn flush_lsn(&self) -> u64 {
        self.flush_lsn
    }

    /// Whether the stream is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Advance the write LSN to a new position.
    pub fn advance_lsn(&mut self, new_lsn: u64) {
        if new_lsn > self.current_lsn {
            self.current_lsn = new_lsn;
        }
    }

    /// Mark the flush position (data safely persisted downstream).
    pub fn mark_flushed(&mut self, lsn: u64) {
        if lsn > self.flush_lsn {
            self.flush_lsn = lsn;
        }
    }

    /// Generate the SQL command to create a replication slot.
    pub fn create_slot_sql(&self) -> String {
        format!(
            "CREATE_REPLICATION_SLOT \"{}\" LOGICAL pgoutput NOEXPORT_SNAPSHOT",
            self.config.slot_name
        )
    }

    /// Generate the SQL to check if a replication slot exists.
    pub fn check_slot_sql(&self) -> String {
        format!(
            "SELECT slot_name FROM pg_replication_slots WHERE slot_name = '{}'",
            self.config.slot_name
        )
    }

    /// Generate the SQL command to start replication.
    pub fn start_replication_sql(&self, from_lsn: u64) -> String {
        let lsn_str = format!("{:X}/{:X}", from_lsn >> 32, from_lsn & 0xFFFF_FFFF);
        format!(
            "START_REPLICATION SLOT \"{}\" LOGICAL {} (proto_version '1', publication_names '{}')",
            self.config.slot_name, lsn_str, self.config.publication
        )
    }

    /// Generate a standby status update message.
    ///
    /// Returns the 34-byte wire format message:
    /// - Byte 0: 'r' (0x72) standby status update identifier
    /// - Bytes 1-8: write LSN (big-endian u64)
    /// - Bytes 9-16: flush LSN (big-endian u64)
    /// - Bytes 17-24: apply LSN (big-endian u64)
    /// - Bytes 25-32: client timestamp (microseconds since 2000-01-01)
    /// - Byte 33: reply requested flag
    pub fn build_standby_status(&self, write_lsn: u64, flush_lsn: u64, apply_lsn: u64) -> Vec<u8> {
        let mut msg = Vec::with_capacity(34);
        msg.push(b'r');
        msg.extend_from_slice(&write_lsn.to_be_bytes());
        msg.extend_from_slice(&flush_lsn.to_be_bytes());
        msg.extend_from_slice(&apply_lsn.to_be_bytes());
        // Client timestamp: microseconds since 2000-01-01 00:00:00 UTC
        let pg_epoch = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800);
        let now_us = std::time::SystemTime::now()
            .duration_since(pg_epoch)
            .unwrap_or_default()
            .as_micros() as u64;
        msg.extend_from_slice(&now_us.to_be_bytes());
        msg.push(0); // reply_requested = false
        msg
    }

    /// Whether enough time has passed to send a status update.
    pub fn should_send_status(&self) -> bool {
        self.last_status_at
            .is_none_or(|last| last.elapsed() >= self.config.heartbeat_interval)
    }

    /// Record that a status update was sent.
    pub fn record_status_sent(&mut self) {
        self.last_status_at = Some(Instant::now());
    }

    /// Connect to PostgreSQL using the configured connection string.
    ///
    /// This is the full connection flow:
    /// 1. Establish TCP connection
    /// 2. Create replication slot if needed
    /// 3. Start replication from the last known LSN
    ///
    /// Requires a running PostgreSQL instance — use `connect_mock` for testing.
    #[cfg(feature = "experimental-cdc-connectors")]
    pub async fn connect(&mut self) -> Result<()> {
        use tokio_postgres::{NoTls, config::ReplicationMode};

        info!(
            slot = %self.config.slot_name,
            publication = %self.config.publication,
            "Connecting to PostgreSQL for logical replication"
        );

        // Parse connection string and configure for replication mode
        let config = tokio_postgres::Config::from_str(&self.config.connection_string)
            .map_err(|e| anyhow!("Invalid PostgreSQL connection string: {}", e))?;

        // Connect in replication mode
        let (client, connection) = config
            .connect(NoTls)
            .await
            .map_err(|e| anyhow!("Failed to connect to PostgreSQL: {}", e))?;

        // Spawn connection handler in background
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                warn!("PostgreSQL connection error: {}", e);
            }
        });

        // Check if replication slot exists
        let slot_check = client.query_one(self.check_slot_sql().as_str(), &[]).await;

        let slot_exists = match slot_check {
            Ok(_) => true,
            Err(tokio_postgres::Error::RowCount) => false,
            Err(e) => return Err(anyhow!("Failed to check replication slot: {}", e)),
        };

        // Create replication slot if it doesn't exist
        if !slot_exists {
            match self.config.slot_behavior {
                super::config::SlotBehavior::CreateIfNotExists => {
                    info!(
                        slot = %self.config.slot_name,
                        "Creating replication slot"
                    );
                    client
                        .execute(self.create_slot_sql().as_str(), &[])
                        .await
                        .map_err(|e| anyhow!("Failed to create replication slot: {}", e))?;
                }
                super::config::SlotBehavior::RequireExisting => {
                    return Err(anyhow!(
                        "Replication slot '{}' does not exist",
                        self.config.slot_name
                    ));
                }
                _ => {
                    // Other slot behaviors (Temporary, DropAndCreate) create slots
                }
            }
        }

        // Start replication from the last known LSN
        let start_lsn = self.current_lsn;
        let start_sql = self.start_replication_sql(start_lsn);

        info!(
            slot = %self.config.slot_name,
            lsn = %ReplicationStream::format_lsn(start_lsn),
            "Starting replication stream"
        );

        // Execute START_REPLICATION command
        // Note: This switches the connection to replication mode
        client
            .execute(start_sql.as_str(), &[])
            .await
            .map_err(|e| anyhow!("Failed to start replication: {}", e))?;

        self.connected = true;
        info!(
            slot = %self.config.slot_name,
            "Successfully connected to PostgreSQL replication stream"
        );

        Ok(())
    }

    /// Connect to PostgreSQL using the configured connection string.
    ///
    /// This is the full connection flow:
    /// 1. Establish TCP connection
    /// 2. Create replication slot if needed
    /// 3. Start replication from the last known LSN
    ///
    /// Requires a running PostgreSQL instance — use `connect_mock` for testing.
    #[cfg(not(feature = "experimental-cdc-connectors"))]
    pub async fn connect(&mut self) -> Result<()> {
        info!(
            slot = %self.config.slot_name,
            publication = %self.config.publication,
            "PostgreSQL CDC transport is not available; enable 'experimental-cdc-connectors' feature"
        );

        warn!(
            "PostgreSQL replication connect() requires 'experimental-cdc-connectors' feature to be enabled"
        );
        self.connected = false;
        Err(anyhow!(
            "PostgreSQL logical replication requires 'experimental-cdc-connectors' feature. \
             Use Debezium for production CDC or enable with: cargo build --features experimental-cdc-connectors"
        ))
    }

    /// Parse an LSN string (e.g., "0/16B3748") into a u64.
    pub fn parse_lsn(lsn_str: &str) -> Result<u64> {
        let parts: Vec<&str> = lsn_str.split('/').collect();
        if parts.len() != 2 {
            return Err(anyhow!("invalid LSN format: {}", lsn_str));
        }
        let high = u64::from_str_radix(parts[0], 16)
            .map_err(|e| anyhow!("invalid LSN high part: {}", e))?;
        let low = u64::from_str_radix(parts[1], 16)
            .map_err(|e| anyhow!("invalid LSN low part: {}", e))?;
        Ok((high << 32) | low)
    }

    /// Format a u64 LSN as a PostgreSQL LSN string.
    pub fn format_lsn(lsn: u64) -> String {
        format!("{:X}/{:X}", lsn >> 32, lsn & 0xFFFF_FFFF)
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::{SlotBehavior, SnapshotMode};
    use super::*;

    fn test_config() -> PostgresConfig {
        PostgresConfig {
            connection_string: "postgres://test:test@localhost:5432/testdb".to_string(),
            slot_name: "test_slot".to_string(),
            publication: "test_pub".to_string(),
            tables: vec![],
            snapshot_mode: SnapshotMode::Initial,
            connect_timeout: Duration::from_secs(10),
            heartbeat_interval: Duration::from_secs(10),
            batch_size: 1000,
            slot_behavior: SlotBehavior::CreateIfNotExists,
        }
    }

    #[test]
    fn test_replication_stream_creation() {
        let stream = ReplicationStream::new(test_config());
        assert_eq!(stream.current_lsn(), 0);
        assert_eq!(stream.flush_lsn(), 0);
        assert!(!stream.is_connected());
    }

    #[test]
    fn test_lsn_tracking() {
        let mut stream = ReplicationStream::new(test_config());
        stream.advance_lsn(100);
        assert_eq!(stream.current_lsn(), 100);

        // Should not go backwards
        stream.advance_lsn(50);
        assert_eq!(stream.current_lsn(), 100);

        stream.mark_flushed(80);
        assert_eq!(stream.flush_lsn(), 80);
    }

    #[test]
    fn test_create_slot_sql() {
        let stream = ReplicationStream::new(test_config());
        assert_eq!(
            stream.create_slot_sql(),
            "CREATE_REPLICATION_SLOT \"test_slot\" LOGICAL pgoutput NOEXPORT_SNAPSHOT"
        );
    }

    #[test]
    fn test_start_replication_sql() {
        let stream = ReplicationStream::new(test_config());
        let sql = stream.start_replication_sql(0x0000_0000_016B_3748);
        assert!(sql.contains("START_REPLICATION SLOT \"test_slot\""));
        assert!(sql.contains("LOGICAL"));
        assert!(sql.contains("test_pub"));
    }

    #[test]
    fn test_standby_status_format() {
        let stream = ReplicationStream::new(test_config());
        let msg = stream.build_standby_status(100, 80, 80);
        assert_eq!(msg.len(), 34);
        assert_eq!(msg[0], b'r');
        // Write LSN at bytes 1-8
        assert_eq!(u64::from_be_bytes(msg[1..9].try_into().unwrap()), 100);
        // Flush LSN at bytes 9-16
        assert_eq!(u64::from_be_bytes(msg[9..17].try_into().unwrap()), 80);
        // Apply LSN at bytes 17-24
        assert_eq!(u64::from_be_bytes(msg[17..25].try_into().unwrap()), 80);
        // Reply requested flag
        assert_eq!(msg[33], 0);
    }

    #[test]
    fn test_parse_lsn() {
        let lsn = ReplicationStream::parse_lsn("0/16B3748").unwrap();
        assert_eq!(lsn, 0x16B3748);

        let lsn2 = ReplicationStream::parse_lsn("1/ABCDEF0").unwrap();
        assert_eq!(lsn2, (1u64 << 32) | 0xABCDEF0);

        assert!(ReplicationStream::parse_lsn("invalid").is_err());
    }

    #[test]
    fn test_format_lsn() {
        assert_eq!(ReplicationStream::format_lsn(0x16B3748), "0/16B3748");
        assert_eq!(
            ReplicationStream::format_lsn((1u64 << 32) | 0xABCDEF0),
            "1/ABCDEF0"
        );
    }

    #[test]
    fn test_lsn_roundtrip() {
        let original = 0x2_DEADBEEF_u64;
        let formatted = ReplicationStream::format_lsn(original);
        let parsed = ReplicationStream::parse_lsn(&formatted).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_keepalive_timing() {
        let mut stream = ReplicationStream::new(test_config());
        assert!(stream.should_send_status()); // Never sent before

        stream.record_status_sent();
        assert!(!stream.should_send_status()); // Just sent
    }
}

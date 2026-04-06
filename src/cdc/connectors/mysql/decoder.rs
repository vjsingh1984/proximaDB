/*
 * Copyright 2025 Vijaykumar Singh
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

//! MySQL binlog event decoder
//!
//! This module decodes MySQL binary log events for CDC processing.
//!
//! ## Binlog Event Types
//!
//! - TABLE_MAP_EVENT (19): Maps table ID to schema
//! - WRITE_ROWS_EVENT (30): INSERT operations
//! - UPDATE_ROWS_EVENT (31): UPDATE operations
//! - DELETE_ROWS_EVENT (32): DELETE operations
//! - QUERY_EVENT (2): DDL statements
//! - XID_EVENT (16): Transaction commit
//! - GTID_EVENT (33): GTID information

use std::collections::HashMap;
use std::io::{self, Cursor, Read};

use crate::cdc::error::{CdcError, CdcResult};
use serde_json;

/// Helper trait for reading values
trait ReadExt {
    fn read_u8_val(&mut self) -> io::Result<u8>;
    fn read_u16_le(&mut self) -> io::Result<u16>;
    fn read_u32_le(&mut self) -> io::Result<u32>;
    fn read_u64_le(&mut self) -> io::Result<u64>;
    fn read_packed_int(&mut self) -> io::Result<u64>;
}

impl<R: Read> ReadExt for R {
    fn read_u8_val(&mut self) -> io::Result<u8> {
        let mut buf = [0u8; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    fn read_u16_le(&mut self) -> io::Result<u16> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf)?;
        Ok(u16::from_le_bytes(buf))
    }

    fn read_u32_le(&mut self) -> io::Result<u32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn read_u64_le(&mut self) -> io::Result<u64> {
        let mut buf = [0u8; 8];
        self.read_exact(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    fn read_packed_int(&mut self) -> io::Result<u64> {
        let first = self.read_u8_val()?;
        match first {
            0..=250 => Ok(first as u64),
            252 => Ok(self.read_u16_le()? as u64),
            253 => {
                let mut buf = [0u8; 3];
                self.read_exact(&mut buf)?;
                Ok(u32::from_le_bytes([buf[0], buf[1], buf[2], 0]) as u64)
            }
            254 => self.read_u64_le(),
            _ => Ok(0),
        }
    }
}

/// Binlog event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EventType {
    /// Unknown or unrecognized event type
    Unknown = 0,
    /// Server startup event (type code 1)
    StartEvent = 1,
    /// SQL statement event for DDL and non-row-based DML (type code 2)
    QueryEvent = 2,
    /// Server shutdown event (type code 3)
    StopEvent = 3,
    /// Binlog rotation event pointing to the next log file (type code 4)
    RotateEvent = 4,
    /// Integer variable assignment event (type code 5)
    IntvarEvent = 5,
    /// LOAD DATA INFILE event (type code 6)
    LoadEvent = 6,
    /// Slave-generated event (type code 7)
    SlaveEvent = 7,
    /// File creation event for LOAD DATA (type code 8)
    CreateFileEvent = 8,
    /// Block append event for LOAD DATA (type code 9)
    AppendBlockEvent = 9,
    /// Execute LOAD DATA event (type code 10)
    ExecLoadEvent = 10,
    /// File deletion event for LOAD DATA (type code 11)
    DeleteFileEvent = 11,
    /// New-style LOAD DATA event (type code 12)
    NewLoadEvent = 12,
    /// RAND() seed value event (type code 13)
    RandEvent = 13,
    /// User-defined variable assignment event (type code 14)
    UserVarEvent = 14,
    /// Format description event describing the binlog version (type code 15)
    FormatDescriptionEvent = 15,
    /// XA transaction identifier commit event (type code 16)
    XidEvent = 16,
    /// Begin block for LOAD DATA statement (type code 17)
    BeginLoadQueryEvent = 17,
    /// Execute a previously loaded LOAD DATA block (type code 18)
    ExecuteLoadQueryEvent = 18,
    /// Maps a table ID to a database and table name for row events (type code 19)
    TableMapEvent = 19,
    /// V1 write (INSERT) rows event (type code 23)
    WriteRowsEventV1 = 23,
    /// V1 update rows event (type code 24)
    UpdateRowsEventV1 = 24,
    /// V1 delete rows event (type code 25)
    DeleteRowsEventV1 = 25,
    /// V2 write (INSERT) rows event (type code 30)
    WriteRowsEvent = 30,
    /// V2 update rows event (type code 31)
    UpdateRowsEvent = 31,
    /// V2 delete rows event (type code 32)
    DeleteRowsEvent = 32,
    /// Global Transaction Identifier event (type code 33)
    GtidEvent = 33,
    /// Anonymous GTID event for transactions without a GTID (type code 34)
    AnonymousGtidEvent = 34,
    /// Set of previous GTIDs covered by earlier binlog files (type code 35)
    PreviousGtidsEvent = 35,
}

impl From<u8> for EventType {
    fn from(value: u8) -> Self {
        match value {
            1 => EventType::StartEvent,
            2 => EventType::QueryEvent,
            3 => EventType::StopEvent,
            4 => EventType::RotateEvent,
            5 => EventType::IntvarEvent,
            15 => EventType::FormatDescriptionEvent,
            16 => EventType::XidEvent,
            19 => EventType::TableMapEvent,
            23 => EventType::WriteRowsEventV1,
            24 => EventType::UpdateRowsEventV1,
            25 => EventType::DeleteRowsEventV1,
            30 => EventType::WriteRowsEvent,
            31 => EventType::UpdateRowsEvent,
            32 => EventType::DeleteRowsEvent,
            33 => EventType::GtidEvent,
            34 => EventType::AnonymousGtidEvent,
            35 => EventType::PreviousGtidsEvent,
            _ => EventType::Unknown,
        }
    }
}

/// Binlog decoder
#[derive(Debug, Default)]
pub struct BinlogDecoder {
    /// Table map cache (table_id -> TableMapEvent)
    table_maps: HashMap<u64, TableMapEvent>,
    /// Current GTID
    current_gtid: Option<String>,
}

impl BinlogDecoder {
    /// Create a new decoder
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a binlog event
    pub fn decode(&mut self, data: &[u8]) -> CdcResult<Option<BinlogEvent>> {
        if data.len() < 19 {
            return Err(CdcError::Serialization("Event too short".to_string()));
        }

        let mut cursor = Cursor::new(data);

        // Event header (19 bytes for MySQL 5.x)
        let timestamp = cursor.read_u32_le()?;
        let event_type = cursor.read_u8_val()?;
        let server_id = cursor.read_u32_le()?;
        let event_length = cursor.read_u32_le()?;
        let next_position = cursor.read_u32_le()?;
        let flags = cursor.read_u16_le()?;

        let event_type = EventType::from(event_type);

        // Read event data
        let header_len = 19;
        let data_len = event_length as usize - header_len;
        let mut event_data = vec![0u8; data_len.min(data.len() - header_len)];
        cursor.read_exact(&mut event_data)?;

        let header = EventHeader {
            timestamp,
            event_type,
            server_id,
            event_length,
            next_position,
            flags,
        };

        self.decode_event(header, &event_data)
    }

    /// Decode event based on type
    fn decode_event(&mut self, header: EventHeader, data: &[u8]) -> CdcResult<Option<BinlogEvent>> {
        match header.event_type {
            EventType::TableMapEvent => {
                let table_map = self.decode_table_map(data)?;
                self.table_maps
                    .insert(table_map.table_id, table_map.clone());
                Ok(Some(BinlogEvent::TableMap(table_map)))
            }
            EventType::WriteRowsEvent | EventType::WriteRowsEventV1 => {
                let row_event = self.decode_row_event(data, RowEventType::Insert)?;
                Ok(Some(BinlogEvent::WriteRows(row_event)))
            }
            EventType::UpdateRowsEvent | EventType::UpdateRowsEventV1 => {
                let row_event = self.decode_row_event(data, RowEventType::Update)?;
                Ok(Some(BinlogEvent::UpdateRows(row_event)))
            }
            EventType::DeleteRowsEvent | EventType::DeleteRowsEventV1 => {
                let row_event = self.decode_row_event(data, RowEventType::Delete)?;
                Ok(Some(BinlogEvent::DeleteRows(row_event)))
            }
            EventType::XidEvent => {
                let mut cursor = Cursor::new(data);
                let xid = cursor.read_u64_le()?;
                Ok(Some(BinlogEvent::Xid { xid }))
            }
            EventType::GtidEvent => {
                let gtid = self.decode_gtid(data)?;
                self.current_gtid = Some(gtid.clone());
                Ok(Some(BinlogEvent::Gtid { gtid }))
            }
            EventType::QueryEvent => {
                let query = self.decode_query(data)?;
                Ok(Some(BinlogEvent::Query(query)))
            }
            EventType::RotateEvent => {
                let (filename, position) = self.decode_rotate(data)?;
                Ok(Some(BinlogEvent::Rotate { filename, position }))
            }
            _ => Ok(None),
        }
    }

    /// Decode TABLE_MAP_EVENT
    fn decode_table_map(&self, data: &[u8]) -> CdcResult<TableMapEvent> {
        let mut cursor = Cursor::new(data);

        let table_id = cursor.read_u64_le()? & 0xFFFFFFFFFFFF; // 6 bytes
        cursor.read_u16_le()?; // flags

        let schema_len = cursor.read_u8_val()? as usize;
        let mut schema = vec![0u8; schema_len];
        cursor.read_exact(&mut schema)?;
        cursor.read_u8_val()?; // null terminator

        let table_len = cursor.read_u8_val()? as usize;
        let mut table = vec![0u8; table_len];
        cursor.read_exact(&mut table)?;
        cursor.read_u8_val()?; // null terminator

        let column_count = cursor.read_packed_int()? as usize;

        // Column types
        let mut column_types = vec![0u8; column_count];
        cursor.read_exact(&mut column_types)?;

        // Column metadata length
        let _metadata_len = cursor.read_packed_int()?;

        // Column definitions (simplified)
        let columns: Vec<ColumnDef> = column_types
            .iter()
            .enumerate()
            .map(|(i, &col_type)| ColumnDef {
                index: i,
                column_type: ColumnType::from(col_type),
                is_nullable: true, // Simplified
                name: None,
            })
            .collect();

        Ok(TableMapEvent {
            table_id,
            schema: String::from_utf8_lossy(&schema).to_string(),
            table: String::from_utf8_lossy(&table).to_string(),
            columns,
        })
    }

    /// Decode row event (WRITE/UPDATE/DELETE)
    fn decode_row_event(&self, data: &[u8], event_type: RowEventType) -> CdcResult<RowEvent> {
        let mut cursor = Cursor::new(data);

        let table_id = cursor.read_u64_le()? & 0xFFFFFFFFFFFF; // 6 bytes
        cursor.read_u16_le()?; // flags

        // Extra data length (for v2 events)
        let extra_len = cursor.read_u16_le()? as usize;
        if extra_len > 2 {
            let mut extra = vec![0u8; extra_len - 2];
            cursor.read_exact(&mut extra)?;
        }

        let _column_count = cursor.read_packed_int()?;

        // Get table map for schema information
        let table_map = self.table_maps.get(&table_id);

        // Columns present bitmap (simplified - assume all columns)
        // In a full implementation, we'd parse the bitmap and row data

        Ok(RowEvent {
            table_id,
            event_type,
            table_map: table_map.cloned(),
            rows: Vec::new(), // Rows would be parsed here
        })
    }

    /// Decode GTID event
    fn decode_gtid(&self, data: &[u8]) -> CdcResult<String> {
        if data.len() < 42 {
            return Ok(String::new());
        }

        let mut cursor = Cursor::new(data);
        cursor.read_u8_val()?; // commit flag

        // UUID (16 bytes)
        let mut uuid = [0u8; 16];
        cursor.read_exact(&mut uuid)?;

        // GNO (8 bytes)
        let gno = cursor.read_u64_le()?;

        // Format UUID
        let uuid_str = format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            uuid[0],
            uuid[1],
            uuid[2],
            uuid[3],
            uuid[4],
            uuid[5],
            uuid[6],
            uuid[7],
            uuid[8],
            uuid[9],
            uuid[10],
            uuid[11],
            uuid[12],
            uuid[13],
            uuid[14],
            uuid[15]
        );

        Ok(format!("{}:{}", uuid_str, gno))
    }

    /// Decode QUERY event
    fn decode_query(&self, data: &[u8]) -> CdcResult<QueryEvent> {
        let mut cursor = Cursor::new(data);

        let _thread_id = cursor.read_u32_le()?;
        let _exec_time = cursor.read_u32_le()?;
        let schema_len = cursor.read_u8_val()? as usize;
        let _error_code = cursor.read_u16_le()?;
        let status_vars_len = cursor.read_u16_le()? as usize;

        // Skip status vars
        let mut status_vars = vec![0u8; status_vars_len];
        cursor.read_exact(&mut status_vars)?;

        // Read schema
        let mut schema = vec![0u8; schema_len];
        cursor.read_exact(&mut schema)?;
        cursor.read_u8_val()?; // null terminator

        // Rest is the query
        let pos = cursor.position() as usize;
        let query = String::from_utf8_lossy(&data[pos..]).to_string();

        Ok(QueryEvent {
            schema: String::from_utf8_lossy(&schema).to_string(),
            query,
        })
    }

    /// Decode ROTATE event
    fn decode_rotate(&self, data: &[u8]) -> CdcResult<(String, u64)> {
        let mut cursor = Cursor::new(data);

        let position = cursor.read_u64_le()?;
        let pos = cursor.position() as usize;
        let filename = String::from_utf8_lossy(&data[pos..])
            .trim_end_matches('\0')
            .to_string();

        Ok((filename, position))
    }

    /// Get table map for a table ID
    pub fn get_table_map(&self, table_id: u64) -> Option<&TableMapEvent> {
        self.table_maps.get(&table_id)
    }

    /// Get current GTID
    pub fn current_gtid(&self) -> Option<&str> {
        self.current_gtid.as_deref()
    }

    /// Clear table maps
    pub fn clear_table_maps(&mut self) {
        self.table_maps.clear();
    }
}

/// Event header
#[derive(Debug, Clone)]
pub struct EventHeader {
    /// Unix timestamp (seconds) when the event was created on the source server
    pub timestamp: u32,
    /// Type of the binlog event
    pub event_type: EventType,
    /// ID of the MySQL server that generated this event
    pub server_id: u32,
    /// Total length of the event in bytes, including the header
    pub event_length: u32,
    /// Byte offset in the binlog file immediately after this event
    pub next_position: u32,
    /// Event flags bitfield (e.g. LOG_EVENT_BINLOG_IN_USE_F)
    pub flags: u16,
}

/// Decoded binlog event
#[derive(Debug, Clone)]
pub enum BinlogEvent {
    /// Table map event
    TableMap(TableMapEvent),
    /// Write (INSERT) rows event
    WriteRows(RowEvent),
    /// Update rows event
    UpdateRows(RowEvent),
    /// Delete rows event
    DeleteRows(RowEvent),
    /// XID (transaction commit)
    Xid {
        /// XA transaction ID that was committed
        xid: u64,
    },
    /// GTID event
    Gtid {
        /// Global Transaction Identifier string in `uuid:gno` format
        gtid: String,
    },
    /// Query event
    Query(QueryEvent),
    /// Rotate event (new binlog file)
    Rotate {
        /// Name of the next binlog file
        filename: String,
        /// Byte position in the new binlog file to start reading from
        position: u64,
    },
}

impl BinlogEvent {
    /// Get the event type for CDC conversion
    pub fn event_type(&self) -> Option<BinlogEventType> {
        match self {
            BinlogEvent::WriteRows(_) => Some(BinlogEventType::WriteRowsEvent),
            BinlogEvent::UpdateRows(_) => Some(BinlogEventType::UpdateRowsEvent),
            BinlogEvent::DeleteRows(_) => Some(BinlogEventType::DeleteRowsEvent),
            _ => None,
        }
    }

    /// Get the database name
    pub fn database(&self) -> String {
        match self {
            BinlogEvent::WriteRows(e) => e
                .table_map
                .as_ref()
                .map(|t| t.schema.clone())
                .unwrap_or_default(),
            BinlogEvent::UpdateRows(e) => e
                .table_map
                .as_ref()
                .map(|t| t.schema.clone())
                .unwrap_or_default(),
            BinlogEvent::DeleteRows(e) => e
                .table_map
                .as_ref()
                .map(|t| t.schema.clone())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// Get the table name
    pub fn table_name(&self) -> Option<String> {
        match self {
            BinlogEvent::WriteRows(e) => e.table_map.as_ref().map(|t| t.table.clone()),
            BinlogEvent::UpdateRows(e) => e.table_map.as_ref().map(|t| t.table.clone()),
            BinlogEvent::DeleteRows(e) => e.table_map.as_ref().map(|t| t.table.clone()),
            _ => None,
        }
    }

    /// Get the row ID (primary key or first column)
    pub fn row_id(&self) -> Option<String> {
        match self {
            BinlogEvent::WriteRows(e) => e.rows.first().and_then(|r| {
                r.after
                    .as_ref()
                    .and_then(|cols| cols.first().and_then(|c| c.as_string()))
            }),
            BinlogEvent::UpdateRows(e) => e.rows.first().and_then(|r| {
                r.before
                    .as_ref()
                    .and_then(|cols| cols.first().and_then(|c| c.as_string()))
                    .or_else(|| {
                        r.after
                            .as_ref()
                            .and_then(|cols| cols.first().and_then(|c| c.as_string()))
                    })
            }),
            BinlogEvent::DeleteRows(e) => e.rows.first().and_then(|r| {
                r.before
                    .as_ref()
                    .and_then(|cols| cols.first().and_then(|c| c.as_string()))
            }),
            _ => None,
        }
    }

    /// Get the position in the binlog
    pub fn position(&self) -> Option<u64> {
        match self {
            BinlogEvent::Rotate { position, .. } => Some(*position),
            _ => None,
        }
    }

    /// Get the row data as JSON
    pub fn row_data(&self) -> Option<serde_json::Value> {
        match self {
            BinlogEvent::WriteRows(e) => rows_to_json(&e.rows, &e.table_map),
            BinlogEvent::UpdateRows(e) => rows_to_json(&e.rows, &e.table_map),
            BinlogEvent::DeleteRows(e) => rows_to_json(&e.rows, &e.table_map),
            _ => None,
        }
    }
}

/// Helper to convert rows to JSON
fn rows_to_json(rows: &[RowData], table_map: &Option<TableMapEvent>) -> Option<serde_json::Value> {
    if rows.is_empty() {
        return None;
    }

    // Get the first row
    let row = &rows[0];

    // Use "after" for INSERT/UPDATE, "before" for DELETE
    let columns = row.after.as_ref().or(row.before.as_ref())?;

    let empty_columns = vec![];
    let columns_def = table_map.as_ref().map_or(&empty_columns, |t| &t.columns);

    let mut obj = serde_json::map::Map::new();
    for (i, col) in columns.iter().enumerate() {
        let col_name = columns_def
            .get(i)
            .and_then(|c| c.name.as_ref())
            .cloned()
            .unwrap_or_else(|| format!("col_{}", i));

        let value = match col {
            ColumnValue::Null => serde_json::Value::Null,
            ColumnValue::String(s) => serde_json::json!(s),
            ColumnValue::Int(i) => serde_json::json!(i),
            ColumnValue::UInt(u) => serde_json::json!(u),
            ColumnValue::Float(f) => serde_json::json!(f),
            ColumnValue::Bytes(b) => serde_json::json!(b),
            ColumnValue::Json(j) => j.clone(),
        };
        obj.insert(col_name, value);
    }

    Some(serde_json::Value::Object(obj))
}

/// Simplified binlog event type for CDC conversion
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinlogEventType {
    /// Corresponds to a WRITE_ROWS event (INSERT operation)
    WriteRowsEvent,
    /// Corresponds to an UPDATE_ROWS event (UPDATE operation)
    UpdateRowsEvent,
    /// Corresponds to a DELETE_ROWS event (DELETE operation)
    DeleteRowsEvent,
}

/// Table map event
#[derive(Debug, Clone)]
pub struct TableMapEvent {
    /// Numeric table identifier assigned by the MySQL server for this binlog sequence
    pub table_id: u64,
    /// Name of the database (schema) that owns the table
    pub schema: String,
    /// Name of the table
    pub table: String,
    /// Ordered list of column definitions for this table
    pub columns: Vec<ColumnDef>,
}

impl TableMapEvent {
    /// Get full table name
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.schema, self.table)
    }
}

/// Row event (INSERT/UPDATE/DELETE)
#[derive(Debug, Clone)]
pub struct RowEvent {
    /// Numeric table identifier matching the associated `TableMapEvent`
    pub table_id: u64,
    /// Whether this event represents an insert, update, or delete
    pub event_type: RowEventType,
    /// Cached table map providing schema metadata; `None` if not yet received
    pub table_map: Option<TableMapEvent>,
    /// Individual row changes carried by this event
    pub rows: Vec<RowData>,
}

/// Row event type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowEventType {
    /// Row was newly inserted (WRITE_ROWS event)
    Insert,
    /// Existing row was modified (UPDATE_ROWS event)
    Update,
    /// Row was removed (DELETE_ROWS event)
    Delete,
}

/// Row data
#[derive(Debug, Clone)]
pub struct RowData {
    /// Column values of the row before the change; present for UPDATE and DELETE events
    pub before: Option<Vec<ColumnValue>>,
    /// Column values of the row after the change; present for INSERT and UPDATE events
    pub after: Option<Vec<ColumnValue>>,
}

/// Column definition
#[derive(Debug, Clone)]
pub struct ColumnDef {
    /// Zero-based position of this column within the table
    pub index: usize,
    /// MySQL wire type for this column
    pub column_type: ColumnType,
    /// Whether the column allows NULL values
    pub is_nullable: bool,
    /// Optional column name; populated when available from schema metadata
    pub name: Option<String>,
}

/// MySQL column types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    /// Fixed-point DECIMAL type (type code 0)
    Decimal,
    /// 1-byte integer TINYINT (type code 1)
    Tiny,
    /// 2-byte integer SMALLINT (type code 2)
    Short,
    /// 4-byte integer INT (type code 3)
    Long,
    /// 4-byte floating-point FLOAT (type code 4)
    Float,
    /// 8-byte floating-point DOUBLE (type code 5)
    Double,
    /// NULL column placeholder (type code 6)
    Null,
    /// TIMESTAMP without fractional seconds (type code 7)
    Timestamp,
    /// 8-byte integer BIGINT (type code 8)
    LongLong,
    /// 3-byte integer MEDIUMINT (type code 9)
    Int24,
    /// Calendar date DATE (type code 10)
    Date,
    /// Time-of-day TIME without fractional seconds (type code 11)
    Time,
    /// Date and time DATETIME without fractional seconds (type code 12)
    DateTime,
    /// Calendar year YEAR (type code 13)
    Year,
    /// Internal new-style DATE representation (type code 14)
    NewDate,
    /// Variable-length string VARCHAR (type code 15)
    Varchar,
    /// Bit-field BIT (type code 16)
    Bit,
    /// TIMESTAMP with fractional-second precision (type code 17)
    Timestamp2,
    /// DATETIME with fractional-second precision (type code 18)
    DateTime2,
    /// TIME with fractional-second precision (type code 19)
    Time2,
    /// JSON column (type code 245)
    Json,
    /// Fixed-precision decimal DECIMAL / NUMERIC (type code 246)
    NewDecimal,
    /// ENUM string column (type code 247)
    Enum,
    /// SET string column (type code 248)
    Set,
    /// TINYBLOB / TINYTEXT (type code 249)
    TinyBlob,
    /// MEDIUMBLOB / MEDIUMTEXT (type code 250)
    MediumBlob,
    /// LONGBLOB / LONGTEXT (type code 251)
    LongBlob,
    /// BLOB / TEXT (type code 252)
    Blob,
    /// Internal variable-length string representation (type code 253)
    VarString,
    /// Fixed-length CHAR / BINARY string (type code 254)
    String,
    /// Spatial GEOMETRY column (type code 255)
    Geometry,
    /// Unrecognized or reserved type code
    Unknown,
}

impl From<u8> for ColumnType {
    fn from(value: u8) -> Self {
        match value {
            0 => ColumnType::Decimal,
            1 => ColumnType::Tiny,
            2 => ColumnType::Short,
            3 => ColumnType::Long,
            4 => ColumnType::Float,
            5 => ColumnType::Double,
            6 => ColumnType::Null,
            7 => ColumnType::Timestamp,
            8 => ColumnType::LongLong,
            9 => ColumnType::Int24,
            10 => ColumnType::Date,
            11 => ColumnType::Time,
            12 => ColumnType::DateTime,
            13 => ColumnType::Year,
            15 => ColumnType::Varchar,
            16 => ColumnType::Bit,
            17 => ColumnType::Timestamp2,
            18 => ColumnType::DateTime2,
            19 => ColumnType::Time2,
            245 => ColumnType::Json,
            246 => ColumnType::NewDecimal,
            247 => ColumnType::Enum,
            248 => ColumnType::Set,
            249 => ColumnType::TinyBlob,
            250 => ColumnType::MediumBlob,
            251 => ColumnType::LongBlob,
            252 => ColumnType::Blob,
            253 => ColumnType::VarString,
            254 => ColumnType::String,
            255 => ColumnType::Geometry,
            _ => ColumnType::Unknown,
        }
    }
}

/// Column value
#[derive(Debug, Clone)]
pub enum ColumnValue {
    /// SQL NULL — the column has no value
    Null,
    /// Signed 64-bit integer value
    Int(i64),
    /// Unsigned 64-bit integer value
    UInt(u64),
    /// 64-bit IEEE 754 floating-point value
    Float(f64),
    /// UTF-8 text string value
    String(String),
    /// Raw byte sequence (e.g. BLOB or BINARY columns)
    Bytes(Vec<u8>),
    /// Structured JSON value
    Json(serde_json::Value),
}

impl ColumnValue {
    /// Try to convert column value to string
    pub fn as_string(&self) -> Option<String> {
        match self {
            ColumnValue::Null => None,
            ColumnValue::Int(i) => Some(i.to_string()),
            ColumnValue::UInt(u) => Some(u.to_string()),
            ColumnValue::Float(f) => Some(f.to_string()),
            ColumnValue::String(s) => Some(s.clone()),
            ColumnValue::Bytes(b) => {
                // Try to convert bytes to UTF-8 string
                String::from_utf8(b.clone()).ok()
            }
            ColumnValue::Json(j) => Some(j.to_string()),
        }
    }
}

/// Query event
#[derive(Debug, Clone)]
pub struct QueryEvent {
    /// Database context in which the query was executed
    pub schema: String,
    /// SQL statement text (DDL or non-row-based DML)
    pub query: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decoder_creation() {
        let decoder = BinlogDecoder::new();
        assert!(decoder.table_maps.is_empty());
        assert!(decoder.current_gtid.is_none());
    }

    #[test]
    fn test_event_type_conversion() {
        assert_eq!(EventType::from(19), EventType::TableMapEvent);
        assert_eq!(EventType::from(30), EventType::WriteRowsEvent);
        assert_eq!(EventType::from(31), EventType::UpdateRowsEvent);
        assert_eq!(EventType::from(32), EventType::DeleteRowsEvent);
        assert_eq!(EventType::from(33), EventType::GtidEvent);
        assert_eq!(EventType::from(255), EventType::Unknown);
    }

    #[test]
    fn test_column_type_conversion() {
        assert_eq!(ColumnType::from(1), ColumnType::Tiny);
        assert_eq!(ColumnType::from(3), ColumnType::Long);
        assert_eq!(ColumnType::from(8), ColumnType::LongLong);
        assert_eq!(ColumnType::from(253), ColumnType::VarString);
        assert_eq!(ColumnType::from(245), ColumnType::Json);
    }

    #[test]
    fn test_row_event_type() {
        assert_eq!(RowEventType::Insert, RowEventType::Insert);
        assert_ne!(RowEventType::Insert, RowEventType::Update);
    }

    #[test]
    fn test_table_map_full_name() {
        let table_map = TableMapEvent {
            table_id: 123,
            schema: "mydb".to_string(),
            table: "users".to_string(),
            columns: vec![],
        };

        assert_eq!(table_map.full_name(), "mydb.users");
    }

    #[test]
    fn test_column_value_variants() {
        let values = vec![
            ColumnValue::Null,
            ColumnValue::Int(-42),
            ColumnValue::UInt(42),
            ColumnValue::Float(3.14),
            ColumnValue::String("hello".to_string()),
            ColumnValue::Bytes(vec![1, 2, 3]),
        ];

        assert_eq!(values.len(), 6);
    }

    #[test]
    fn test_query_event() {
        let query = QueryEvent {
            schema: "test".to_string(),
            query: "CREATE TABLE foo (id INT)".to_string(),
        };

        assert_eq!(query.schema, "test");
        assert!(query.query.contains("CREATE TABLE"));
    }

    #[test]
    fn test_clear_table_maps() {
        let mut decoder = BinlogDecoder::new();
        decoder.table_maps.insert(
            1,
            TableMapEvent {
                table_id: 1,
                schema: "db".to_string(),
                table: "t1".to_string(),
                columns: vec![],
            },
        );

        assert!(!decoder.table_maps.is_empty());
        decoder.clear_table_maps();
        assert!(decoder.table_maps.is_empty());
    }

    #[test]
    fn test_decode_short_data() {
        let mut decoder = BinlogDecoder::new();
        let result = decoder.decode(&[0u8; 10]);
        assert!(result.is_err());
    }
}

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

use serde_json;
use crate::cdc::error::{CdcError, CdcResult};

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
    Unknown = 0,
    StartEvent = 1,
    QueryEvent = 2,
    StopEvent = 3,
    RotateEvent = 4,
    IntvarEvent = 5,
    LoadEvent = 6,
    SlaveEvent = 7,
    CreateFileEvent = 8,
    AppendBlockEvent = 9,
    ExecLoadEvent = 10,
    DeleteFileEvent = 11,
    NewLoadEvent = 12,
    RandEvent = 13,
    UserVarEvent = 14,
    FormatDescriptionEvent = 15,
    XidEvent = 16,
    BeginLoadQueryEvent = 17,
    ExecuteLoadQueryEvent = 18,
    TableMapEvent = 19,
    WriteRowsEventV1 = 23,
    UpdateRowsEventV1 = 24,
    DeleteRowsEventV1 = 25,
    WriteRowsEvent = 30,
    UpdateRowsEvent = 31,
    DeleteRowsEvent = 32,
    GtidEvent = 33,
    AnonymousGtidEvent = 34,
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
    pub timestamp: u32,
    pub event_type: EventType,
    pub server_id: u32,
    pub event_length: u32,
    pub next_position: u32,
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
    Xid { xid: u64 },
    /// GTID event
    Gtid { gtid: String },
    /// Query event
    Query(QueryEvent),
    /// Rotate event (new binlog file)
    Rotate { filename: String, position: u64 },
}

impl BinlogEvent {
    /// Get the event type for CDC conversion
    pub fn event_type(&self) -> Option<BinlogEventType> {
        match self {
            BinlogEvent::WriteRows(_) => Some(BinlogEventType::WRITE_ROWS_EVENT),
            BinlogEvent::UpdateRows(_) => Some(BinlogEventType::UPDATE_ROWS_EVENT),
            BinlogEvent::DeleteRows(_) => Some(BinlogEventType::DELETE_ROWS_EVENT),
            _ => None,
        }
    }

    /// Get the database name
    pub fn database(&self) -> String {
        match self {
            BinlogEvent::WriteRows(e) => e.table_map.as_ref()
                .map(|t| t.schema.clone())
                .unwrap_or_default(),
            BinlogEvent::UpdateRows(e) => e.table_map.as_ref()
                .map(|t| t.schema.clone())
                .unwrap_or_default(),
            BinlogEvent::DeleteRows(e) => e.table_map.as_ref()
                .map(|t| t.schema.clone())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// Get the table name
    pub fn table_name(&self) -> Option<String> {
        match self {
            BinlogEvent::WriteRows(e) => e.table_map.as_ref()
                .map(|t| t.table.clone()),
            BinlogEvent::UpdateRows(e) => e.table_map.as_ref()
                .map(|t| t.table.clone()),
            BinlogEvent::DeleteRows(e) => e.table_map.as_ref()
                .map(|t| t.table.clone()),
            _ => None,
        }
    }

    /// Get the row ID (primary key or first column)
    pub fn row_id(&self) -> Option<String> {
        match self {
            BinlogEvent::WriteRows(e) => e.rows.first().and_then(|r| {
                r.after.as_ref().and_then(|cols| cols.first().and_then(|c| c.as_string()))
            }),
            BinlogEvent::UpdateRows(e) => e.rows.first().and_then(|r| {
                r.before.as_ref().and_then(|cols| cols.first().and_then(|c| c.as_string()))
                    .or_else(|| r.after.as_ref().and_then(|cols| cols.first().and_then(|c| c.as_string())))
            }),
            BinlogEvent::DeleteRows(e) => e.rows.first().and_then(|r| {
                r.before.as_ref().and_then(|cols| cols.first().and_then(|c| c.as_string()))
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
    let columns_def = table_map.as_ref().map(|t| &t.columns).unwrap_or(&empty_columns);

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
    WRITE_ROWS_EVENT,
    UPDATE_ROWS_EVENT,
    DELETE_ROWS_EVENT,
}

/// Table map event
#[derive(Debug, Clone)]
pub struct TableMapEvent {
    pub table_id: u64,
    pub schema: String,
    pub table: String,
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
    pub table_id: u64,
    pub event_type: RowEventType,
    pub table_map: Option<TableMapEvent>,
    pub rows: Vec<RowData>,
}

/// Row event type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowEventType {
    Insert,
    Update,
    Delete,
}

/// Row data
#[derive(Debug, Clone)]
pub struct RowData {
    pub before: Option<Vec<ColumnValue>>,
    pub after: Option<Vec<ColumnValue>>,
}

/// Column definition
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub index: usize,
    pub column_type: ColumnType,
    pub is_nullable: bool,
    pub name: Option<String>,
}

/// MySQL column types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Decimal,
    Tiny,
    Short,
    Long,
    Float,
    Double,
    Null,
    Timestamp,
    LongLong,
    Int24,
    Date,
    Time,
    DateTime,
    Year,
    NewDate,
    Varchar,
    Bit,
    Timestamp2,
    DateTime2,
    Time2,
    Json,
    NewDecimal,
    Enum,
    Set,
    TinyBlob,
    MediumBlob,
    LongBlob,
    Blob,
    VarString,
    String,
    Geometry,
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
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
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
    pub schema: String,
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

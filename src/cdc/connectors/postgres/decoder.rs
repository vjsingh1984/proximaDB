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

//! PostgreSQL pgoutput protocol decoder
//!
//! This module implements decoding of the pgoutput logical replication protocol.
//!
//! ## Protocol Messages
//!
//! - `B` (0x42): Begin transaction
//! - `C` (0x43): Commit transaction
//! - `I` (0x49): Insert
//! - `U` (0x55): Update
//! - `D` (0x44): Delete
//! - `T` (0x54): Truncate
//! - `R` (0x52): Relation (table definition)
//! - `Y` (0x59): Type
//! - `O` (0x4F): Origin

use std::collections::HashMap;
use std::io::{self, Cursor, Read};

use crate::cdc::error::{CdcError, CdcResult};

/// Helper trait for reading big-endian values from a cursor
trait ReadBigEndian {
    fn read_u8_be(&mut self) -> io::Result<u8>;
    fn read_u16_be(&mut self) -> io::Result<u16>;
    fn read_u32_be(&mut self) -> io::Result<u32>;
    fn read_u64_be(&mut self) -> io::Result<u64>;
    fn read_i32_be(&mut self) -> io::Result<i32>;
    fn read_i64_be(&mut self) -> io::Result<i64>;
}

impl<R: Read> ReadBigEndian for R {
    fn read_u8_be(&mut self) -> io::Result<u8> {
        let mut buf = [0u8; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    fn read_u16_be(&mut self) -> io::Result<u16> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf)?;
        Ok(u16::from_be_bytes(buf))
    }

    fn read_u32_be(&mut self) -> io::Result<u32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_be_bytes(buf))
    }

    fn read_u64_be(&mut self) -> io::Result<u64> {
        let mut buf = [0u8; 8];
        self.read_exact(&mut buf)?;
        Ok(u64::from_be_bytes(buf))
    }

    fn read_i32_be(&mut self) -> io::Result<i32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(i32::from_be_bytes(buf))
    }

    fn read_i64_be(&mut self) -> io::Result<i64> {
        let mut buf = [0u8; 8];
        self.read_exact(&mut buf)?;
        Ok(i64::from_be_bytes(buf))
    }
}

/// Decoder for PostgreSQL pgoutput protocol
#[derive(Debug, Default)]
pub struct PgOutputDecoder {
    /// Relation cache (relation_id -> relation)
    relations: HashMap<u32, PgRelation>,
    /// Current transaction
    current_tx: Option<TransactionState>,
}

impl PgOutputDecoder {
    /// Create a new decoder
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a pgoutput message
    pub fn decode(&mut self, data: &[u8]) -> CdcResult<Vec<PgOutputEvent>> {
        if data.is_empty() {
            return Ok(Vec::new());
        }

        let mut cursor = Cursor::new(data);
        let mut events = Vec::new();

        while (cursor.position() as usize) < data.len() {
            let message_type = cursor.read_u8_be()?;

            let event = match message_type {
                b'B' => self.decode_begin(&mut cursor)?,
                b'C' => self.decode_commit(&mut cursor)?,
                b'I' => self.decode_insert(&mut cursor)?,
                b'U' => self.decode_update(&mut cursor)?,
                b'D' => self.decode_delete(&mut cursor)?,
                b'T' => self.decode_truncate(&mut cursor)?,
                b'R' => self.decode_relation(&mut cursor)?,
                b'Y' => self.decode_type(&mut cursor)?,
                b'O' => self.decode_origin(&mut cursor)?,
                _ => {
                    // Skip unknown message types
                    continue;
                }
            };

            if let Some(e) = event {
                events.push(e);
            }
        }

        Ok(events)
    }

    /// Decode Begin message
    fn decode_begin(&mut self, cursor: &mut Cursor<&[u8]>) -> CdcResult<Option<PgOutputEvent>> {
        let final_lsn = cursor.read_u64_be()?;
        let commit_time = cursor.read_i64_be()?;
        let xid = cursor.read_u32_be()?;

        self.current_tx = Some(TransactionState {
            xid,
            final_lsn,
            commit_time,
            event_count: 0,
        });

        Ok(Some(PgOutputEvent::Begin {
            final_lsn,
            commit_time,
            xid,
        }))
    }

    /// Decode Commit message
    fn decode_commit(&mut self, cursor: &mut Cursor<&[u8]>) -> CdcResult<Option<PgOutputEvent>> {
        let _flags = cursor.read_u8_be()?;
        let commit_lsn = cursor.read_u64_be()?;
        let end_lsn = cursor.read_u64_be()?;
        let commit_time = cursor.read_i64_be()?;

        let tx = self.current_tx.take();

        Ok(Some(PgOutputEvent::Commit {
            commit_lsn,
            end_lsn,
            commit_time,
            xid: tx.map(|t| t.xid),
        }))
    }

    /// Decode Insert message
    fn decode_insert(&mut self, cursor: &mut Cursor<&[u8]>) -> CdcResult<Option<PgOutputEvent>> {
        let relation_id = cursor.read_u32_be()?;
        let _new_tuple_type = cursor.read_u8_be()?; // Always 'N'
        let tuple = self.decode_tuple(cursor, relation_id)?;

        let relation = self.relations.get(&relation_id).cloned();

        if let Some(ref mut tx) = self.current_tx {
            tx.event_count += 1;
        }

        Ok(Some(PgOutputEvent::Insert {
            relation_id,
            relation,
            tuple,
        }))
    }

    /// Decode Update message
    fn decode_update(&mut self, cursor: &mut Cursor<&[u8]>) -> CdcResult<Option<PgOutputEvent>> {
        let relation_id = cursor.read_u32_be()?;

        // Check for key/old tuple
        let indicator = cursor.read_u8_be()?;
        let old_tuple = if indicator == b'K' || indicator == b'O' {
            Some(self.decode_tuple(cursor, relation_id)?)
        } else {
            None
        };

        // New tuple is always present
        let new_indicator = if old_tuple.is_some() {
            cursor.read_u8_be()?
        } else {
            indicator
        };

        let new_tuple = if new_indicator == b'N' {
            Some(self.decode_tuple(cursor, relation_id)?)
        } else {
            None
        };

        let relation = self.relations.get(&relation_id).cloned();

        if let Some(ref mut tx) = self.current_tx {
            tx.event_count += 1;
        }

        Ok(Some(PgOutputEvent::Update {
            relation_id,
            relation,
            old_tuple,
            new_tuple,
        }))
    }

    /// Decode Delete message
    fn decode_delete(&mut self, cursor: &mut Cursor<&[u8]>) -> CdcResult<Option<PgOutputEvent>> {
        let relation_id = cursor.read_u32_be()?;
        let key_type = cursor.read_u8_be()?; // 'K' for key, 'O' for old tuple

        let key_tuple = self.decode_tuple(cursor, relation_id)?;

        let relation = self.relations.get(&relation_id).cloned();

        if let Some(ref mut tx) = self.current_tx {
            tx.event_count += 1;
        }

        Ok(Some(PgOutputEvent::Delete {
            relation_id,
            relation,
            key_tuple,
            is_key_only: key_type == b'K',
        }))
    }

    /// Decode Truncate message
    fn decode_truncate(&mut self, cursor: &mut Cursor<&[u8]>) -> CdcResult<Option<PgOutputEvent>> {
        let n_relations = cursor.read_u32_be()?;
        let options = cursor.read_u8_be()?;

        let mut relation_ids = Vec::with_capacity(n_relations as usize);
        for _ in 0..n_relations {
            relation_ids.push(cursor.read_u32_be()?);
        }

        let cascade = (options & 0x01) != 0;
        let restart_identity = (options & 0x02) != 0;

        Ok(Some(PgOutputEvent::Truncate {
            relation_ids,
            cascade,
            restart_identity,
        }))
    }

    /// Decode Relation message
    fn decode_relation(&mut self, cursor: &mut Cursor<&[u8]>) -> CdcResult<Option<PgOutputEvent>> {
        let relation_id = cursor.read_u32_be()?;
        let namespace = self.read_cstring(cursor)?;
        let name = self.read_cstring(cursor)?;
        let replica_identity = cursor.read_u8_be()?;
        let n_columns = cursor.read_u16_be()?;

        let mut columns = Vec::with_capacity(n_columns as usize);
        for _ in 0..n_columns {
            let flags = cursor.read_u8_be()?;
            let column_name = self.read_cstring(cursor)?;
            let type_oid = cursor.read_u32_be()?;
            let type_modifier = cursor.read_i32_be()?;

            columns.push(PgColumn {
                name: column_name,
                type_oid,
                type_modifier,
                is_key: (flags & 0x01) != 0,
            });
        }

        let relation = PgRelation {
            id: relation_id,
            namespace: namespace.clone(),
            name: name.clone(),
            replica_identity,
            columns,
        };

        self.relations.insert(relation_id, relation.clone());

        Ok(Some(PgOutputEvent::Relation(relation)))
    }

    /// Decode Type message
    fn decode_type(&mut self, cursor: &mut Cursor<&[u8]>) -> CdcResult<Option<PgOutputEvent>> {
        let type_oid = cursor.read_u32_be()?;
        let namespace = self.read_cstring(cursor)?;
        let name = self.read_cstring(cursor)?;

        Ok(Some(PgOutputEvent::Type {
            type_oid,
            namespace,
            name,
        }))
    }

    /// Decode Origin message
    fn decode_origin(&mut self, cursor: &mut Cursor<&[u8]>) -> CdcResult<Option<PgOutputEvent>> {
        let origin_lsn = cursor.read_u64_be()?;
        let origin_name = self.read_cstring(cursor)?;

        Ok(Some(PgOutputEvent::Origin {
            origin_lsn,
            origin_name,
        }))
    }

    /// Decode a tuple
    fn decode_tuple(&self, cursor: &mut Cursor<&[u8]>, relation_id: u32) -> CdcResult<TupleData> {
        let n_columns = cursor.read_u16_be()?;
        let mut values = Vec::with_capacity(n_columns as usize);

        let column_names: Vec<String> = self
            .relations
            .get(&relation_id)
            .map(|r| r.columns.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();

        for i in 0..n_columns {
            let column_type = cursor.read_u8_be()?;
            let value = match column_type {
                b'n' => ColumnValue::Null,
                b'u' => ColumnValue::Unchanged,
                b't' => {
                    let len = cursor.read_u32_be()? as usize;
                    let mut data = vec![0u8; len];
                    cursor.read_exact(&mut data)?;
                    // pgoutput sends text format
                    ColumnValue::Text(String::from_utf8_lossy(&data).to_string())
                }
                b'b' => {
                    let len = cursor.read_u32_be()? as usize;
                    let mut data = vec![0u8; len];
                    cursor.read_exact(&mut data)?;
                    ColumnValue::Binary(data)
                }
                _ => {
                    return Err(CdcError::Serialization(format!(
                        "Unknown column type: {}",
                        column_type as char
                    )));
                }
            };

            let name = column_names.get(i as usize).cloned();
            values.push((name, value));
        }

        Ok(TupleData { values })
    }

    /// Read a null-terminated string
    fn read_cstring(&self, cursor: &mut Cursor<&[u8]>) -> io::Result<String> {
        let mut bytes = Vec::new();
        loop {
            let byte = cursor.read_u8_be()?;
            if byte == 0 {
                break;
            }
            bytes.push(byte);
        }
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// Get a relation by ID
    pub fn get_relation(&self, relation_id: u32) -> Option<&PgRelation> {
        self.relations.get(&relation_id)
    }

    /// Clear relation cache
    pub fn clear_relations(&mut self) {
        self.relations.clear();
    }
}

/// Transaction state tracking
#[derive(Debug)]
struct TransactionState {
    xid: u32,
    final_lsn: u64,
    commit_time: i64,
    event_count: u32,
}

/// Decoded pgoutput event
#[derive(Debug, Clone)]
pub enum PgOutputEvent {
    /// Begin transaction
    Begin {
        final_lsn: u64,
        commit_time: i64,
        xid: u32,
    },
    /// Commit transaction
    Commit {
        commit_lsn: u64,
        end_lsn: u64,
        commit_time: i64,
        xid: Option<u32>,
    },
    /// Insert row
    Insert {
        relation_id: u32,
        relation: Option<PgRelation>,
        tuple: TupleData,
    },
    /// Update row
    Update {
        relation_id: u32,
        relation: Option<PgRelation>,
        old_tuple: Option<TupleData>,
        new_tuple: Option<TupleData>,
    },
    /// Delete row
    Delete {
        relation_id: u32,
        relation: Option<PgRelation>,
        key_tuple: TupleData,
        is_key_only: bool,
    },
    /// Truncate table
    Truncate {
        relation_ids: Vec<u32>,
        cascade: bool,
        restart_identity: bool,
    },
    /// Relation definition
    Relation(PgRelation),
    /// Type definition
    Type {
        type_oid: u32,
        namespace: String,
        name: String,
    },
    /// Origin
    Origin {
        origin_lsn: u64,
        origin_name: String,
    },
}

/// PostgreSQL relation (table) definition
#[derive(Debug, Clone)]
pub struct PgRelation {
    /// Relation OID
    pub id: u32,
    /// Schema name
    pub namespace: String,
    /// Table name
    pub name: String,
    /// Replica identity setting
    pub replica_identity: u8,
    /// Column definitions
    pub columns: Vec<PgColumn>,
}

impl PgRelation {
    /// Get fully qualified table name
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.namespace, self.name)
    }

    /// Get column by name
    pub fn get_column(&self, name: &str) -> Option<&PgColumn> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Get column index by name
    pub fn get_column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }
}

/// PostgreSQL column definition
#[derive(Debug, Clone)]
pub struct PgColumn {
    /// Column name
    pub name: String,
    /// Type OID
    pub type_oid: u32,
    /// Type modifier
    pub type_modifier: i32,
    /// Is this part of the replica identity (key)
    pub is_key: bool,
}

/// Tuple data (row values)
#[derive(Debug, Clone)]
pub struct TupleData {
    /// Column values with optional names
    pub values: Vec<(Option<String>, ColumnValue)>,
}

impl TupleData {
    /// Get value by column name
    pub fn get(&self, name: &str) -> Option<&ColumnValue> {
        self.values
            .iter()
            .find(|(n, _)| n.as_ref().is_some_and(|s| s == name))
            .map(|(_, v)| v)
    }

    /// Get value by index
    pub fn get_by_index(&self, index: usize) -> Option<&ColumnValue> {
        self.values.get(index).map(|(_, v)| v)
    }

    /// Convert to a map
    pub fn to_map(&self) -> HashMap<String, ColumnValue> {
        self.values
            .iter()
            .filter_map(|(name, value)| name.clone().map(|n| (n, value.clone())))
            .collect()
    }
}

/// Column value types
#[derive(Debug, Clone)]
pub enum ColumnValue {
    /// NULL value
    Null,
    /// Unchanged (for REPLICA IDENTITY)
    Unchanged,
    /// Text format value
    Text(String),
    /// Binary format value
    Binary(Vec<u8>),
}

impl ColumnValue {
    /// Check if value is null
    pub fn is_null(&self) -> bool {
        matches!(self, ColumnValue::Null)
    }

    /// Get as string if text
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ColumnValue::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Get as bytes if binary
    pub fn as_binary(&self) -> Option<&[u8]> {
        match self {
            ColumnValue::Binary(b) => Some(b),
            _ => None,
        }
    }

    /// Parse as i64
    pub fn parse_i64(&self) -> Option<i64> {
        match self {
            ColumnValue::Text(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// Parse as f64
    pub fn parse_f64(&self) -> Option<f64> {
        match self {
            ColumnValue::Text(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// Parse as JSON value
    pub fn parse_json(&self) -> Option<serde_json::Value> {
        match self {
            ColumnValue::Text(s) => serde_json::from_str(s).ok(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decoder_creation() {
        let decoder = PgOutputDecoder::new();
        assert!(decoder.relations.is_empty());
    }

    #[test]
    fn test_column_value_null() {
        let value = ColumnValue::Null;
        assert!(value.is_null());
        assert!(value.as_text().is_none());
    }

    #[test]
    fn test_column_value_text() {
        let value = ColumnValue::Text("hello".to_string());
        assert!(!value.is_null());
        assert_eq!(value.as_text(), Some("hello"));
    }

    #[test]
    fn test_column_value_parse() {
        let int_value = ColumnValue::Text("42".to_string());
        assert_eq!(int_value.parse_i64(), Some(42));

        let float_value = ColumnValue::Text("3.14".to_string());
        assert!((float_value.parse_f64().unwrap() - 3.14).abs() < 0.001);

        let json_value = ColumnValue::Text(r#"{"key": "value"}"#.to_string());
        let parsed = json_value.parse_json().unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn test_tuple_data() {
        let tuple = TupleData {
            values: vec![
                (Some("id".to_string()), ColumnValue::Text("1".to_string())),
                (
                    Some("name".to_string()),
                    ColumnValue::Text("test".to_string()),
                ),
            ],
        };

        assert!(tuple.get("id").is_some());
        assert!(tuple.get("name").is_some());
        assert!(tuple.get("unknown").is_none());
        assert!(tuple.get_by_index(0).is_some());
    }

    #[test]
    fn test_tuple_to_map() {
        let tuple = TupleData {
            values: vec![
                (Some("id".to_string()), ColumnValue::Text("1".to_string())),
                (
                    Some("name".to_string()),
                    ColumnValue::Text("test".to_string()),
                ),
            ],
        };

        let map = tuple.to_map();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("id"));
        assert!(map.contains_key("name"));
    }

    #[test]
    fn test_pg_relation() {
        let relation = PgRelation {
            id: 123,
            namespace: "public".to_string(),
            name: "users".to_string(),
            replica_identity: 0,
            columns: vec![
                PgColumn {
                    name: "id".to_string(),
                    type_oid: 23,
                    type_modifier: -1,
                    is_key: true,
                },
                PgColumn {
                    name: "name".to_string(),
                    type_oid: 25,
                    type_modifier: -1,
                    is_key: false,
                },
            ],
        };

        assert_eq!(relation.full_name(), "public.users");
        assert!(relation.get_column("id").is_some());
        assert_eq!(relation.get_column_index("name"), Some(1));
    }

    #[test]
    fn test_decode_empty() {
        let mut decoder = PgOutputDecoder::new();
        let events = decoder.decode(&[]).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_clear_relations() {
        let mut decoder = PgOutputDecoder::new();
        decoder.relations.insert(
            1,
            PgRelation {
                id: 1,
                namespace: "public".to_string(),
                name: "test".to_string(),
                replica_identity: 0,
                columns: vec![],
            },
        );

        assert!(!decoder.relations.is_empty());
        decoder.clear_relations();
        assert!(decoder.relations.is_empty());
    }
}

//! JSON value serialization utilities for index statistics and filter expressions
//!
//! This module provides efficient serialization for serde_json::Value types
//! used in index statistics (min/max values) and filter expression evaluation.

use anyhow::Result;
use serde_json::Value;
use std::io::{Read, Write};

/// Serialize a serde_json::Value with type tag for deserialization
pub fn serialize_json_value<W: Write>(value: &Value, writer: &mut W) -> Result<()> {
    match value {
        Value::Null => {
            writer.write_all(&[0u8])?; // Type tag for Null
        }
        Value::Bool(b) => {
            writer.write_all(&[1u8])?; // Type tag for Bool
            writer.write_all(&[if *b { 1u8 } else { 0u8 }])?;
        }
        Value::Number(n) => {
            writer.write_all(&[2u8])?; // Type tag for Number
            if let Some(i) = n.as_i64() {
                writer.write_all(&[0u8])?; // Subtype: i64
                writer.write_all(&i.to_le_bytes())?;
            } else if let Some(u) = n.as_u64() {
                writer.write_all(&[1u8])?; // Subtype: u64
                writer.write_all(&u.to_le_bytes())?;
            } else if let Some(f) = n.as_f64() {
                writer.write_all(&[2u8])?; // Subtype: f64
                writer.write_all(&f.to_le_bytes())?;
            } else {
                return Err(anyhow::anyhow!("Unsupported number type"));
            }
        }
        Value::String(s) => {
            writer.write_all(&[3u8])?; // Type tag for String
            let bytes = s.as_bytes();
            writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
            writer.write_all(bytes)?;
        }
        Value::Array(_) => {
            writer.write_all(&[4u8])?; // Type tag for Array
            // For index statistics, we don't need to support arrays
            return Err(anyhow::anyhow!("Arrays not supported in index statistics"));
        }
        Value::Object(_) => {
            writer.write_all(&[5u8])?; // Type tag for Object
            // For index statistics, we don't need to support objects
            return Err(anyhow::anyhow!("Objects not supported in index statistics"));
        }
    }
    Ok(())
}

/// Deserialize a serde_json::Value with type tag
pub fn deserialize_json_value<R: Read>(reader: &mut R) -> Result<Value> {
    let mut type_tag = [0u8; 1];
    reader.read_exact(&mut type_tag)?;
    
    match type_tag[0] {
        0 => Ok(Value::Null),
        1 => {
            let mut bool_buf = [0u8; 1];
            reader.read_exact(&mut bool_buf)?;
            Ok(Value::Bool(bool_buf[0] != 0))
        }
        2 => {
            let mut subtype = [0u8; 1];
            reader.read_exact(&mut subtype)?;
            match subtype[0] {
                0 => {
                    let mut buf = [0u8; 8];
                    reader.read_exact(&mut buf)?;
                    let i = i64::from_le_bytes(buf);
                    Ok(serde_json::Number::from(i).into())
                }
                1 => {
                    let mut buf = [0u8; 8];
                    reader.read_exact(&mut buf)?;
                    let u = u64::from_le_bytes(buf);
                    Ok(serde_json::Number::from(u).into())
                }
                2 => {
                    let mut buf = [0u8; 8];
                    reader.read_exact(&mut buf)?;
                    let f = f64::from_le_bytes(buf);
                    serde_json::Number::from_f64(f)
                        .map(Value::Number)
                        .ok_or_else(|| anyhow::anyhow!("Invalid f64 value"))
                }
                _ => Err(anyhow::anyhow!("Invalid number subtype")),
            }
        }
        3 => {
            let mut len_buf = [0u8; 4];
            reader.read_exact(&mut len_buf)?;
            let len = u32::from_le_bytes(len_buf) as usize;
            let mut bytes = vec![0u8; len];
            reader.read_exact(&mut bytes)?;
            let s = String::from_utf8(bytes)?;
            Ok(Value::String(s))
        }
        4 => Err(anyhow::anyhow!("Arrays not supported in index statistics")),
        5 => Err(anyhow::anyhow!("Objects not supported in index statistics")),
        _ => Err(anyhow::anyhow!("Invalid type tag: {}", type_tag[0])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_json_value_roundtrip() {
        let test_cases = vec![
            Value::Null,
            Value::Bool(true),
            Value::Bool(false),
            Value::Number(serde_json::Number::from(42i64)),
            Value::Number(serde_json::Number::from(42u64)),
            Value::Number(serde_json::Number::from_f64(3.14).unwrap()),
            Value::String("hello world".to_string()),
        ];
        
        for value in test_cases {
            let mut buffer = Vec::new();
            serialize_json_value(&value, &mut buffer).unwrap();
            
            let mut cursor = std::io::Cursor::new(buffer);
            let deserialized = deserialize_json_value(&mut cursor).unwrap();
            
            assert_eq!(value, deserialized);
        }
    }
}
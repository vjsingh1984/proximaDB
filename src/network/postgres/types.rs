// PostgreSQL type system for wire protocol
//
// Maps PostgreSQL types to ProximaDB types

/// PostgreSQL type
#[derive(Debug, Clone, PartialEq)]
pub enum PgType {
    /// Boolean
    Bool,
    /// Bytea (binary data)
    Bytea,
    /// 64-bit signed integer
    Int8,
    /// 32-bit signed integer
    Int4,
    /// 16-bit signed integer
    Int2,
    /// Text
    Text,
    /// Variable character
    Varchar,
    /// Single-precision float
    Float4,
    /// Double-precision float
    Float8,
    /// Timestamp with time zone
    Timestamptz,
    /// Timestamp without time zone
    Timestamp,
    /// Date
    Date,
    /// JSON
    Json,
    /// JSONB
    Jsonb,
    /// UUID
    Uuid,
    /// Array
    Array(Box<PgType>),
    /// Vector (pgvector extension)
    Vector,
    /// Unknown/unspecified
    Unknown,
}

impl PgType {
    /// Get PostgreSQL OID for this type
    pub fn oid(&self) -> i32 {
        match self {
            PgType::Bool => 16,
            PgType::Bytea => 17,
            PgType::Int8 => 20,
            PgType::Int4 => 23,
            PgType::Int2 => 21,
            PgType::Text => 25,
            PgType::Varchar => 1043,
            PgType::Float4 => 700,
            PgType::Float8 => 701,
            PgType::Timestamptz => 1184,
            PgType::Timestamp => 1114,
            PgType::Date => 1082,
            PgType::Json => 114,
            PgType::Jsonb => 3802,
            PgType::Uuid => 2950,
            PgType::Array(inner) => inner.array_oid(),
            PgType::Vector => 0, // Custom type
            PgType::Unknown => 0,
        }
    }

    /// Get array OID for this type
    fn array_oid(&self) -> i32 {
        match self {
            PgType::Bool => 1000,
            PgType::Int8 => 1016,
            PgType::Int4 => 1007,
            PgType::Int2 => 1005,
            PgType::Text => 1009,
            PgType::Varchar => 1015,
            PgType::Float4 => 1021,
            PgType::Float8 => 1022,
            _ => 0,
        }
    }

    /// Get type size
    pub fn size(&self) -> i16 {
        match self {
            PgType::Bool => 1,
            PgType::Int2 => 2,
            PgType::Int4 => 4,
            PgType::Int8 => 8,
            PgType::Float4 => 4,
            PgType::Float8 => 8,
            PgType::Date => 4,
            PgType::Timestamp => 8,
            PgType::Timestamptz => 8,
            PgType::Uuid => 16,
            _ => -1, // Variable length
        }
    }

    /// Create from OID
    pub fn from_oid(oid: i32) -> Self {
        match oid {
            16 => PgType::Bool,
            17 => PgType::Bytea,
            20 => PgType::Int8,
            21 => PgType::Int2,
            23 => PgType::Int4,
            25 => PgType::Text,
            114 => PgType::Json,
            700 => PgType::Float4,
            701 => PgType::Float8,
            1043 => PgType::Varchar,
            1082 => PgType::Date,
            1114 => PgType::Timestamp,
            1184 => PgType::Timestamptz,
            2950 => PgType::Uuid,
            3802 => PgType::Jsonb,
            // Array types
            1000 => PgType::Array(Box::new(PgType::Bool)),
            1005 => PgType::Array(Box::new(PgType::Int2)),
            1007 => PgType::Array(Box::new(PgType::Int4)),
            1009 => PgType::Array(Box::new(PgType::Text)),
            1015 => PgType::Array(Box::new(PgType::Varchar)),
            1016 => PgType::Array(Box::new(PgType::Int8)),
            1021 => PgType::Array(Box::new(PgType::Float4)),
            1022 => PgType::Array(Box::new(PgType::Float8)),
            _ => PgType::Unknown,
        }
    }

    /// Get type name
    pub fn name(&self) -> &'static str {
        match self {
            PgType::Bool => "bool",
            PgType::Bytea => "bytea",
            PgType::Int8 => "int8",
            PgType::Int4 => "int4",
            PgType::Int2 => "int2",
            PgType::Text => "text",
            PgType::Varchar => "varchar",
            PgType::Float4 => "float4",
            PgType::Float8 => "float8",
            PgType::Timestamptz => "timestamptz",
            PgType::Timestamp => "timestamp",
            PgType::Date => "date",
            PgType::Json => "json",
            PgType::Jsonb => "jsonb",
            PgType::Uuid => "uuid",
            PgType::Array(_) => "array",
            PgType::Vector => "vector",
            PgType::Unknown => "unknown",
        }
    }
}

/// Field description for row descriptions
#[derive(Debug, Clone)]
pub struct FieldDescription {
    /// Field name
    pub name: String,
    /// Table OID (0 if not from a table)
    pub table_oid: i32,
    /// Column number (0 if not from a table)
    pub column_number: i16,
    /// Type OID
    pub type_oid: i32,
    /// Type size
    pub type_size: i16,
    /// Type modifier
    pub type_modifier: i32,
    /// Format code (0 = text, 1 = binary)
    pub format_code: i16,
}

impl FieldDescription {
    /// Create a new field description
    pub fn new(name: &str, pg_type: PgType) -> Self {
        Self {
            name: name.to_string(),
            table_oid: 0,
            column_number: 0,
            type_oid: pg_type.oid(),
            type_size: pg_type.size(),
            type_modifier: -1,
            format_code: 0, // Text format
        }
    }

    /// Create with table reference
    pub fn with_table(mut self, table_oid: i32, column_number: i16) -> Self {
        self.table_oid = table_oid;
        self.column_number = column_number;
        self
    }

    /// Set binary format
    pub fn binary(mut self) -> Self {
        self.format_code = 1;
        self
    }
}

/// Value encoder/decoder for PostgreSQL wire format
pub struct PgValue;

impl PgValue {
    /// Encode a value to text format
    pub fn encode_text(value: &DataValue) -> String {
        match value {
            DataValue::Null => String::new(),
            DataValue::Bool(b) => if *b { "t" } else { "f" }.to_string(),
            DataValue::Int2(i) => i.to_string(),
            DataValue::Int4(i) => i.to_string(),
            DataValue::Int8(i) => i.to_string(),
            DataValue::Float4(f) => f.to_string(),
            DataValue::Float8(f) => f.to_string(),
            DataValue::Text(s) => s.clone(),
            DataValue::Bytea(b) => format!("\\x{}", hex::encode(b)),
            DataValue::Timestamp(t) => t.to_string(),
            DataValue::Date(d) => d.to_string(),
            DataValue::Json(j) => j.clone(),
            DataValue::Uuid(u) => u.clone(),
            DataValue::Array(arr) => {
                let elements: Vec<String> = arr.iter().map(|v| Self::encode_text(v)).collect();
                format!("{{{}}}", elements.join(","))
            }
            DataValue::Vector(v) => {
                let elements: Vec<String> = v.iter().map(|f| f.to_string()).collect();
                format!("[{}]", elements.join(","))
            }
        }
    }

    /// Decode text format to value
    pub fn decode_text(text: &str, pg_type: PgType) -> Result<DataValue, String> {
        if text.is_empty() {
            return Ok(DataValue::Null);
        }

        match pg_type {
            PgType::Bool => {
                let b = text == "t" || text == "true" || text == "1";
                Ok(DataValue::Bool(b))
            }
            PgType::Int2 => text.parse().map(DataValue::Int2).map_err(|e| e.to_string()),
            PgType::Int4 => text.parse().map(DataValue::Int4).map_err(|e| e.to_string()),
            PgType::Int8 => text.parse().map(DataValue::Int8).map_err(|e| e.to_string()),
            PgType::Float4 => text
                .parse()
                .map(DataValue::Float4)
                .map_err(|e| e.to_string()),
            PgType::Float8 => text
                .parse()
                .map(DataValue::Float8)
                .map_err(|e| e.to_string()),
            PgType::Text | PgType::Varchar => Ok(DataValue::Text(text.to_string())),
            PgType::Json | PgType::Jsonb => Ok(DataValue::Json(text.to_string())),
            PgType::Uuid => Ok(DataValue::Uuid(text.to_string())),
            PgType::Vector => {
                // Parse [1.0, 2.0, 3.0] format
                let trimmed = text.trim_start_matches('[').trim_end_matches(']');
                let values: Result<Vec<f32>, _> =
                    trimmed.split(',').map(|s| s.trim().parse()).collect();
                values.map(DataValue::Vector).map_err(|e| e.to_string())
            }
            _ => Ok(DataValue::Text(text.to_string())),
        }
    }
}

/// Data value for type-safe handling
#[derive(Debug, Clone)]
pub enum DataValue {
    /// Null value
    Null,
    /// Boolean
    Bool(bool),
    /// 16-bit integer
    Int2(i16),
    /// 32-bit integer
    Int4(i32),
    /// 64-bit integer
    Int8(i64),
    /// Single-precision float
    Float4(f32),
    /// Double-precision float
    Float8(f64),
    /// Text
    Text(String),
    /// Binary data
    Bytea(Vec<u8>),
    /// Timestamp (as string for simplicity)
    Timestamp(String),
    /// Date (as string)
    Date(String),
    /// JSON
    Json(String),
    /// UUID
    Uuid(String),
    /// Array
    Array(Vec<DataValue>),
    /// Vector
    Vector(Vec<f32>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pg_type_oid() {
        assert_eq!(PgType::Bool.oid(), 16);
        assert_eq!(PgType::Int4.oid(), 23);
        assert_eq!(PgType::Text.oid(), 25);
        assert_eq!(PgType::Float8.oid(), 701);
    }

    #[test]
    fn test_pg_type_from_oid() {
        assert_eq!(PgType::from_oid(16), PgType::Bool);
        assert_eq!(PgType::from_oid(23), PgType::Int4);
        assert_eq!(PgType::from_oid(25), PgType::Text);
    }

    #[test]
    fn test_field_description() {
        let field = FieldDescription::new("id", PgType::Int4);
        assert_eq!(field.name, "id");
        assert_eq!(field.type_oid, 23);
        assert_eq!(field.type_size, 4);
    }

    #[test]
    fn test_encode_text() {
        assert_eq!(PgValue::encode_text(&DataValue::Bool(true)), "t");
        assert_eq!(PgValue::encode_text(&DataValue::Int4(42)), "42");
        assert_eq!(
            PgValue::encode_text(&DataValue::Text("hello".to_string())),
            "hello"
        );
    }

    #[test]
    fn test_decode_text() {
        assert!(matches!(
            PgValue::decode_text("t", PgType::Bool),
            Ok(DataValue::Bool(true))
        ));
        assert!(matches!(
            PgValue::decode_text("42", PgType::Int4),
            Ok(DataValue::Int4(42))
        ));
    }

    #[test]
    fn test_decode_vector() {
        let result = PgValue::decode_text("[1.0, 2.0, 3.0]", PgType::Vector);
        if let Ok(DataValue::Vector(v)) = result {
            assert_eq!(v.len(), 3);
            assert_eq!(v[0], 1.0);
        } else {
            panic!("Expected Vector");
        }
    }
}

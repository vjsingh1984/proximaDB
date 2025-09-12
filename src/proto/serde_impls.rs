// Custom serde implementations for protobuf oneof types
// This allows memory-efficient oneof while maintaining JSON compatibility

use serde::{Serialize, Deserialize, Serializer, Deserializer};
use crate::utils::encoding::{base64_encode, base64_decode};
use crate::proto::proximadb_v1::{SqlValue, sql_value::Value as SqlValueVariant};
use crate::proto::proximadb_v1::{PropertyValue, property_value::Value as PropertyValueVariant};

// Custom serde for SqlValue
impl Serialize for SqlValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        
        match &self.value {
            Some(SqlValueVariant::StringValue(v)) => {
                map.serialize_entry("string_value", v)?;
            }
            Some(SqlValueVariant::NumberValue(v)) => {
                map.serialize_entry("number_value", v)?;
            }
            Some(SqlValueVariant::BoolValue(v)) => {
                map.serialize_entry("bool_value", v)?;
            }
            Some(SqlValueVariant::Int64Value(v)) => {
                map.serialize_entry("int64_value", v)?;
            }
            Some(SqlValueVariant::BytesValue(v)) => {
                map.serialize_entry("bytes_value", &base64_encode(v))?;
            }
            Some(SqlValueVariant::NullValue(_)) => {
                map.serialize_entry("null_value", &serde_json::Value::Null)?;
            }
            Some(SqlValueVariant::ArrayValue(v)) => {
                map.serialize_entry("array_value", v)?;
            }
            Some(SqlValueVariant::ObjectValue(v)) => {
                map.serialize_entry("object_value", v)?;
            }
            None => {
                map.serialize_entry("null_value", &serde_json::Value::Null)?;
            }
        }
        
        map.end()
    }
}

impl<'de> Deserialize<'de> for SqlValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SqlValueHelper {
            string_value: Option<String>,
            number_value: Option<f64>,
            bool_value: Option<bool>,
            int64_value: Option<i64>,
            bytes_value: Option<String>, // base64 encoded
            null_value: Option<serde_json::Value>,
            array_value: Option<crate::proto::proximadb_v1::SqlArray>,
            object_value: Option<crate::proto::proximadb_v1::SqlObject>,
        }
        
        let helper = SqlValueHelper::deserialize(deserializer)?;
        
        let value = if let Some(v) = helper.string_value {
            Some(SqlValueVariant::StringValue(v))
        } else if let Some(v) = helper.number_value {
            Some(SqlValueVariant::NumberValue(v))
        } else if let Some(v) = helper.bool_value {
            Some(SqlValueVariant::BoolValue(v))
        } else if let Some(v) = helper.int64_value {
            Some(SqlValueVariant::Int64Value(v))
        } else if let Some(v) = helper.bytes_value {
            let bytes = base64_decode(&v).map_err(serde::de::Error::custom)?;
            Some(SqlValueVariant::BytesValue(bytes))
        } else if helper.null_value.is_some() {
            Some(SqlValueVariant::NullValue(0)) // prost_types::NullValue
        } else if let Some(v) = helper.array_value {
            Some(SqlValueVariant::ArrayValue(v))
        } else if let Some(v) = helper.object_value {
            Some(SqlValueVariant::ObjectValue(v))
        } else {
            None
        };
        
        Ok(SqlValue { value })
    }
}

// Custom serde for PropertyValue
impl Serialize for PropertyValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        
        match &self.value {
            Some(PropertyValueVariant::StringValue(v)) => {
                map.serialize_entry("string_value", v)?;
            }
            Some(PropertyValueVariant::IntValue(v)) => {
                map.serialize_entry("int_value", v)?;
            }
            Some(PropertyValueVariant::DoubleValue(v)) => {
                map.serialize_entry("double_value", v)?;
            }
            Some(PropertyValueVariant::BoolValue(v)) => {
                map.serialize_entry("bool_value", v)?;
            }
            Some(PropertyValueVariant::BytesValue(v)) => {
                map.serialize_entry("bytes_value", &base64_encode(v))?;
            }
            Some(PropertyValueVariant::ArrayValue(v)) => {
                map.serialize_entry("array_value", v)?;
            }
            Some(PropertyValueVariant::ObjectValue(v)) => {
                map.serialize_entry("object_value", v)?;
            }
            Some(PropertyValueVariant::VectorValue(v)) => {
                map.serialize_entry("vector_value", &v.values)?;
            }
            None => {
                map.serialize_entry("null_value", &serde_json::Value::Null)?;
            }
        }
        
        map.end()
    }
}

impl<'de> Deserialize<'de> for PropertyValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct PropertyValueHelper {
            string_value: Option<String>,
            int_value: Option<i64>,
            double_value: Option<f64>,
            bool_value: Option<bool>,
            bytes_value: Option<String>, // base64 encoded
            array_value: Option<crate::proto::proximadb_v1::PropertyArray>,
            object_value: Option<crate::proto::proximadb_v1::PropertyObject>,
            vector_value: Option<Vec<f32>>,
            null_value: Option<serde_json::Value>,
        }
        
        let helper = PropertyValueHelper::deserialize(deserializer)?;
        
        let value = if let Some(v) = helper.string_value {
            Some(PropertyValueVariant::StringValue(v))
        } else if let Some(v) = helper.int_value {
            Some(PropertyValueVariant::IntValue(v))
        } else if let Some(v) = helper.double_value {
            Some(PropertyValueVariant::DoubleValue(v))
        } else if let Some(v) = helper.bool_value {
            Some(PropertyValueVariant::BoolValue(v))
        } else if let Some(v) = helper.bytes_value {
            let bytes = base64_decode(&v).map_err(serde::de::Error::custom)?;
            Some(PropertyValueVariant::BytesValue(bytes))
        } else if let Some(v) = helper.array_value {
            Some(PropertyValueVariant::ArrayValue(v))
        } else if let Some(v) = helper.object_value {
            Some(PropertyValueVariant::ObjectValue(v))
        } else if let Some(v) = helper.vector_value {
            Some(PropertyValueVariant::VectorValue(crate::proto::proximadb_v1::VectorData {
                values: v,
            }))
        } else {
            None
        };
        
        Ok(PropertyValue { value })
    }
}

// prost automatically provides PartialEq for oneof types - no manual implementation needed

// Add custom serde implementations for other oneof types

// Custom serde for SourceContent
impl Serialize for crate::proto::proximadb_v1::SourceContent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        use crate::proto::proximadb_v1::source_content::Data;
        
        let mut map = serializer.serialize_map(Some(1))?;
        
        match &self.data {
            Some(Data::TextContent(v)) => map.serialize_entry("text_content", v)?,
            Some(Data::BinaryContent(v)) => map.serialize_entry("binary_content", &base64_encode(v))?,
            Some(Data::ExternalReference(v)) => map.serialize_entry("external_reference", v)?,
            None => map.serialize_entry("null_content", &serde_json::Value::Null)?,
        }
        
        map.end()
    }
}

// Custom serde for FilterClause (from entity.proto)
impl Serialize for crate::proto::proximadb_v1::FilterClause {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        use crate::proto::proximadb_v1::filter_clause::Value;
        
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("field", &self.field)?;
        map.serialize_entry("op", &self.op)?;
        
        match &self.value {
            Some(Value::StringValue(v)) => map.serialize_entry("string_value", v)?,
            Some(Value::IntValue(v)) => map.serialize_entry("int_value", v)?,
            Some(Value::DoubleValue(v)) => map.serialize_entry("double_value", v)?,
            Some(Value::BoolValue(v)) => map.serialize_entry("bool_value", v)?,
            None => map.serialize_entry("null_value", &serde_json::Value::Null)?,
        }
        
        map.end()
    }
}

// Custom serde for VectorRecord
impl Serialize for crate::proto::proximadb_v1::VectorRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("VectorRecord", 6)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("vector", &self.vector)?;
        state.serialize_field("metadata", &self.metadata)?;
        state.serialize_field("timestamp", &self.timestamp)?;
        state.serialize_field("updated_at", &self.updated_at)?;
        state.serialize_field("expires_at", &self.expires_at)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::VectorRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Id,
            Vector,
            Metadata,
            Timestamp,
            UpdatedAt,
            ExpiresAt,
        }

        struct VectorRecordVisitor;

        impl<'de> serde::de::Visitor<'de> for VectorRecordVisitor {
            type Value = crate::proto::proximadb_v1::VectorRecord;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct VectorRecord")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::VectorRecord, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut id = None;
                let mut vector = None;
                let mut metadata = None;
                let mut timestamp = None;
                let mut updated_at = None;
                let mut expires_at = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Id => {
                            if id.is_some() {
                                return Err(serde::de::Error::duplicate_field("id"));
                            }
                            id = Some(map.next_value()?);
                        }
                        Field::Vector => {
                            if vector.is_some() {
                                return Err(serde::de::Error::duplicate_field("vector"));
                            }
                            vector = Some(map.next_value()?);
                        }
                        Field::Metadata => {
                            if metadata.is_some() {
                                return Err(serde::de::Error::duplicate_field("metadata"));
                            }
                            metadata = Some(map.next_value()?);
                        }
                        Field::Timestamp => {
                            if timestamp.is_some() {
                                return Err(serde::de::Error::duplicate_field("timestamp"));
                            }
                            timestamp = Some(map.next_value()?);
                        }
                        Field::UpdatedAt => {
                            if updated_at.is_some() {
                                return Err(serde::de::Error::duplicate_field("updated_at"));
                            }
                            updated_at = Some(map.next_value()?);
                        }
                        Field::ExpiresAt => {
                            if expires_at.is_some() {
                                return Err(serde::de::Error::duplicate_field("expires_at"));
                            }
                            expires_at = Some(map.next_value()?);
                        }
                    }
                }

                let id = id.ok_or_else(|| serde::de::Error::missing_field("id"))?;
                let vector = vector.ok_or_else(|| serde::de::Error::missing_field("vector"))?;
                let metadata = metadata.ok_or_else(|| serde::de::Error::missing_field("metadata"))?;
                let timestamp = timestamp.ok_or_else(|| serde::de::Error::missing_field("timestamp"))?;

                Ok(crate::proto::proximadb_v1::VectorRecord {
                    id,
                    vector,
                    metadata,
                    timestamp,
                    updated_at,
                    expires_at,
                })
            }
        }

        deserializer.deserialize_struct("VectorRecord", &["id", "vector", "metadata", "timestamp", "updated_at", "expires_at"], VectorRecordVisitor)
    }
}

// Custom serde implementations for protobuf oneof types ONLY
use serde::{Serialize, Deserialize, Serializer, Deserializer};
use crate::utils::encoding::{base64_encode, base64_decode};
use crate::proto::proximadb_v1::{SqlValue, sql_value::Value as SqlValueVariant};
use crate::proto::proximadb_v1::{PropertyValue, property_value::Value as PropertyValueVariant};
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

// Custom serde for TypedField (has oneof value)
impl Serialize for crate::proto::proximadb_v1::TypedField {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("TypedField", 3)?;
        
        // Serialize the oneof value field
        match &self.value {
            Some(crate::proto::proximadb_v1::typed_field::Value::StringValue(v)) => {
                state.serialize_field("value", &serde_json::json!({"string_value": v}))?;
            }
            Some(crate::proto::proximadb_v1::typed_field::Value::IntValue(v)) => {
                state.serialize_field("value", &serde_json::json!({"int_value": v}))?;
            }
            Some(crate::proto::proximadb_v1::typed_field::Value::DoubleValue(v)) => {
                state.serialize_field("value", &serde_json::json!({"double_value": v}))?;
            }
            Some(crate::proto::proximadb_v1::typed_field::Value::BoolValue(v)) => {
                state.serialize_field("value", &serde_json::json!({"bool_value": v}))?;
            }
            Some(crate::proto::proximadb_v1::typed_field::Value::StringArray(v)) => {
                state.serialize_field("value", &serde_json::json!({"string_array": v}))?;
            }
            Some(crate::proto::proximadb_v1::typed_field::Value::TimestampValueMs(v)) => {
                state.serialize_field("value", &serde_json::json!({"timestamp_value_ms": v}))?;
            }
            None => {
                state.serialize_field("value", &serde_json::Value::Null)?;
            }
        }
        
        // Serialize the boolean fields
        state.serialize_field("indexed", &self.indexed)?;
        state.serialize_field("filterable", &self.filterable)?;
        
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::TypedField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct TypedFieldHelper {
            value: Option<serde_json::Value>,
            indexed: Option<bool>,
            filterable: Option<bool>,
        }

        let helper = TypedFieldHelper::deserialize(deserializer)?;

        let value = if let Some(val) = helper.value {
            if let Some(obj) = val.as_object() {
                if let Some(string_val) = obj.get("string_value") {
                    if let Some(s) = string_val.as_str() {
                        Some(crate::proto::proximadb_v1::typed_field::Value::StringValue(s.to_string()))
                    } else { None }
                } else if let Some(int_val) = obj.get("int_value") {
                    if let Some(i) = int_val.as_i64() {
                        Some(crate::proto::proximadb_v1::typed_field::Value::IntValue(i))
                    } else { None }
                } else if let Some(double_val) = obj.get("double_value") {
                    if let Some(d) = double_val.as_f64() {
                        Some(crate::proto::proximadb_v1::typed_field::Value::DoubleValue(d))
                    } else { None }
                } else if let Some(bool_val) = obj.get("bool_value") {
                    if let Some(b) = bool_val.as_bool() {
                        Some(crate::proto::proximadb_v1::typed_field::Value::BoolValue(b))
                    } else { None }
                } else if let Some(timestamp_val) = obj.get("timestamp_value_ms") {
                    if let Some(t) = timestamp_val.as_i64() {
                        Some(crate::proto::proximadb_v1::typed_field::Value::TimestampValueMs(t))
                    } else { None }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        Ok(crate::proto::proximadb_v1::TypedField {
            value,
            indexed: helper.indexed.unwrap_or(false),
            filterable: helper.filterable.unwrap_or(false),
        })
    }
}

// Custom serde for MetadataValue (has oneof value)
impl Serialize for crate::proto::proximadb_v1::MetadataValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;

        match &self.value {
            Some(crate::proto::proximadb_v1::metadata_value::Value::StringValue(v)) => {
                map.serialize_entry("string_value", v)?;
            }
            Some(crate::proto::proximadb_v1::metadata_value::Value::IntValue(v)) => {
                map.serialize_entry("int_value", v)?;
            }
            Some(crate::proto::proximadb_v1::metadata_value::Value::DoubleValue(v)) => {
                map.serialize_entry("double_value", v)?;
            }
            Some(crate::proto::proximadb_v1::metadata_value::Value::BoolValue(v)) => {
                map.serialize_entry("bool_value", v)?;
            }
            None => {
                map.serialize_entry("null_value", &serde_json::Value::Null)?;
            }
        }

        map.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::MetadataValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct MetadataValueHelper {
            string_value: Option<String>,
            int_value: Option<i64>,
            double_value: Option<f64>,
            bool_value: Option<bool>,
            null_value: Option<serde_json::Value>,
        }

        let helper = MetadataValueHelper::deserialize(deserializer)?;

        let value = if let Some(v) = helper.string_value {
            Some(crate::proto::proximadb_v1::metadata_value::Value::StringValue(v))
        } else if let Some(v) = helper.int_value {
            Some(crate::proto::proximadb_v1::metadata_value::Value::IntValue(v))
        } else if let Some(v) = helper.double_value {
            Some(crate::proto::proximadb_v1::metadata_value::Value::DoubleValue(v))
        } else if let Some(v) = helper.bool_value {
            Some(crate::proto::proximadb_v1::metadata_value::Value::BoolValue(v))
        } else {
            None
        };

        Ok(crate::proto::proximadb_v1::MetadataValue { value })
    }
}

// FilterClause Deserialize implementation
impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::FilterClause {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct FilterClauseHelper {
            field: String,
            op: i32,
            string_value: Option<String>,
            int_value: Option<i64>,
            double_value: Option<f64>,
            bool_value: Option<bool>,
            null_value: Option<serde_json::Value>,
        }

        let helper = FilterClauseHelper::deserialize(deserializer)?;

        let value = if let Some(v) = helper.string_value {
            Some(crate::proto::proximadb_v1::filter_clause::Value::StringValue(v))
        } else if let Some(v) = helper.int_value {
            Some(crate::proto::proximadb_v1::filter_clause::Value::IntValue(v))
        } else if let Some(v) = helper.double_value {
            Some(crate::proto::proximadb_v1::filter_clause::Value::DoubleValue(v))
        } else if let Some(v) = helper.bool_value {
            Some(crate::proto::proximadb_v1::filter_clause::Value::BoolValue(v))
        } else {
            None
        };

        Ok(crate::proto::proximadb_v1::FilterClause {
            field: helper.field,
            op: helper.op,
            value,
        })
    }
}

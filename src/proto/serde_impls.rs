// Custom serde implementations for protobuf oneof types ONLY
// Simple types use build.rs auto-derives, complex oneof types use custom implementations

use serde::{Serialize, Deserialize, Serializer, Deserializer};
use crate::utils::encoding::{base64_encode, base64_decode};
use crate::proto::proximadb_v1::{SqlValue, sql_value::Value as SqlValueVariant};
use crate::proto::proximadb_v1::{PropertyValue, property_value::Value as PropertyValueVariant};

// Custom serde for SqlValue (has oneof value)
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
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            StringValue,
            NumberValue,
            BoolValue,
            Int64Value,
            BytesValue,
            NullValue,
            ArrayValue,
            ObjectValue,
        }

        struct SqlValueVisitor;

        impl<'de> serde::de::Visitor<'de> for SqlValueVisitor {
            type Value = SqlValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct SqlValue")
            }

            fn visit_map<V>(self, mut map: V) -> Result<SqlValue, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut value = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::StringValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v: String = map.next_value()?;
                            value = Some(SqlValueVariant::StringValue(v));
                        }
                        Field::NumberValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v: f64 = map.next_value()?;
                            value = Some(SqlValueVariant::NumberValue(v));
                        }
                        Field::BoolValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v: bool = map.next_value()?;
                            value = Some(SqlValueVariant::BoolValue(v));
                        }
                        Field::Int64Value => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v: i64 = map.next_value()?;
                            value = Some(SqlValueVariant::Int64Value(v));
                        }
                        Field::BytesValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v: String = map.next_value()?;
                            let bytes = base64_decode(&v).map_err(serde::de::Error::custom)?;
                            value = Some(SqlValueVariant::BytesValue(bytes));
                        }
                        Field::NullValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let _: serde_json::Value = map.next_value()?;
                            value = Some(SqlValueVariant::NullValue(()));
                        }
                        Field::ArrayValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v = map.next_value()?;
                            value = Some(SqlValueVariant::ArrayValue(v));
                        }
                        Field::ObjectValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v = map.next_value()?;
                            value = Some(SqlValueVariant::ObjectValue(v));
                        }
                    }
                }

                Ok(SqlValue { value })
            }
        }

        deserializer.deserialize_struct("SqlValue", &["string_value", "number_value", "bool_value", "int64_value", "bytes_value", "null_value", "array_value", "object_value"], SqlValueVisitor)
    }
}

// Custom serde for PropertyValue (has oneof value)
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
            Some(PropertyValueVariant::FloatValue(v)) => {
                map.serialize_entry("float_value", v)?;
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
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            StringValue,
            IntValue,
            FloatValue,
            BoolValue,
            BytesValue,
            ArrayValue,
            ObjectValue,
            NullValue,
        }

        struct PropertyValueVisitor;

        impl<'de> serde::de::Visitor<'de> for PropertyValueVisitor {
            type Value = PropertyValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct PropertyValue")
            }

            fn visit_map<V>(self, mut map: V) -> Result<PropertyValue, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut value = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::StringValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v: String = map.next_value()?;
                            value = Some(PropertyValueVariant::StringValue(v));
                        }
                        Field::IntValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v: i64 = map.next_value()?;
                            value = Some(PropertyValueVariant::IntValue(v));
                        }
                        Field::FloatValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v: f64 = map.next_value()?;
                            value = Some(PropertyValueVariant::FloatValue(v));
                        }
                        Field::BoolValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v: bool = map.next_value()?;
                            value = Some(PropertyValueVariant::BoolValue(v));
                        }
                        Field::BytesValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v: String = map.next_value()?;
                            let bytes = base64_decode(&v).map_err(serde::de::Error::custom)?;
                            value = Some(PropertyValueVariant::BytesValue(bytes));
                        }
                        Field::ArrayValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v = map.next_value()?;
                            value = Some(PropertyValueVariant::ArrayValue(v));
                        }
                        Field::ObjectValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let v = map.next_value()?;
                            value = Some(PropertyValueVariant::ObjectValue(v));
                        }
                        Field::NullValue => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let _: serde_json::Value = map.next_value()?;
                            // PropertyValue doesn't have null variant, use default
                        }
                    }
                }

                Ok(PropertyValue { value })
            }
        }

        deserializer.deserialize_struct("PropertyValue", &["string_value", "int_value", "float_value", "bool_value", "bytes_value", "array_value", "object_value"], PropertyValueVisitor)
    }
}

// Custom serde for SourceContent (has oneof data)
impl Serialize for crate::proto::proximadb_v1::SourceContent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;

        match &self.data {
            Some(crate::proto::proximadb_v1::source_content::Data::TextContent(v)) => {
                map.serialize_entry("text_content", v)?;
            }
            Some(crate::proto::proximadb_v1::source_content::Data::BinaryContent(v)) => {
                map.serialize_entry("binary_content", &base64_encode(v))?;
            }
            None => {
                map.serialize_entry("null", &serde_json::Value::Null)?;
            }
        }

        map.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::SourceContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            TextContent,
            BinaryContent,
            Null,
        }

        struct SourceContentVisitor;

        impl<'de> serde::de::Visitor<'de> for SourceContentVisitor {
            type Value = crate::proto::proximadb_v1::SourceContent;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct SourceContent")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::SourceContent, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut data = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::TextContent => {
                            if data.is_some() {
                                return Err(serde::de::Error::duplicate_field("data"));
                            }
                            let v: String = map.next_value()?;
                            data = Some(crate::proto::proximadb_v1::source_content::Data::TextContent(v));
                        }
                        Field::BinaryContent => {
                            if data.is_some() {
                                return Err(serde::de::Error::duplicate_field("data"));
                            }
                            let v: String = map.next_value()?;
                            let bytes = base64_decode(&v).map_err(serde::de::Error::custom)?;
                            data = Some(crate::proto::proximadb_v1::source_content::Data::BinaryContent(bytes));
                        }
                        Field::Null => {
                            let _: serde_json::Value = map.next_value()?;
                            // Leave data as None
                        }
                    }
                }

                Ok(crate::proto::proximadb_v1::SourceContent { data })
            }
        }

        deserializer.deserialize_struct("SourceContent", &["text_content", "binary_content", "null"], SourceContentVisitor)
    }
}
// Custom serde for MetadataItem (has oneof value)
impl Serialize for crate::proto::proximadb_v1::MetadataItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("MetadataItem", 2)?;
        state.serialize_field("key", &self.key)?;

        match &self.value {
            Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(v)) => {
                state.serialize_field("value", &serde_json::json!({"string_value": v}))?;
            }
            Some(crate::proto::proximadb_v1::metadata_item::Value::NumberValue(v)) => {
                state.serialize_field("value", &serde_json::json!({"number_value": v}))?;
            }
            Some(crate::proto::proximadb_v1::metadata_item::Value::BoolValue(v)) => {
                state.serialize_field("value", &serde_json::json!({"bool_value": v}))?;
            }
            None => {
                state.serialize_field("value", &serde_json::Value::Null)?;
            }
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::MetadataItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Key,
            Value,
        }

        struct MetadataItemVisitor;

        impl<'de> serde::de::Visitor<'de> for MetadataItemVisitor {
            type Value = crate::proto::proximadb_v1::MetadataItem;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct MetadataItem")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::MetadataItem, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut key = None;
                let mut value = None;

                while let Some(field) = map.next_key()? {
                    match field {
                        Field::Key => {
                            if key.is_some() {
                                return Err(serde::de::Error::duplicate_field("key"));
                            }
                            key = Some(map.next_value()?);
                        }
                        Field::Value => {
                            if value.is_some() {
                                return Err(serde::de::Error::duplicate_field("value"));
                            }
                            let val: serde_json::Value = map.next_value()?;
                            if let Some(obj) = val.as_object() {
                                if let Some(string_val) = obj.get("string_value") {
                                    if let Some(s) = string_val.as_str() {
                                        value = Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(s.to_string()));
                                    }
                                } else if let Some(number_val) = obj.get("number_value") {
                                    if let Some(n) = number_val.as_f64() {
                                        value = Some(crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n));
                                    }
                                } else if let Some(bool_val) = obj.get("bool_value") {
                                    if let Some(b) = bool_val.as_bool() {
                                        value = Some(crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b));
                                    }
                                }
                            }
                        }
                    }
                }

                let key = key.ok_or_else(|| serde::de::Error::missing_field("key"))?;

                Ok(crate::proto::proximadb_v1::MetadataItem { key, value })
            }
        }

        deserializer.deserialize_struct("MetadataItem", &["key", "value"], MetadataItemVisitor)
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

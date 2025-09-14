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
                    quantized_vector: None,
                    source: String::new(),
                    version: 0,
                })
            }
        }

        deserializer.deserialize_struct("VectorRecord", &["id", "vector", "metadata", "timestamp", "updated_at", "expires_at"], VectorRecordVisitor)
    }
}

// Custom serde for Collection
impl Serialize for crate::proto::proximadb_v1::Collection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Collection", 5)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("config", &self.config)?;
        state.serialize_field("stats", &self.stats)?;
        state.serialize_field("created_at", &self.created_at)?;
        state.serialize_field("updated_at", &self.updated_at)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::Collection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Id,
            Config,
            Stats,
            CreatedAt,
            UpdatedAt,
        }

        struct CollectionVisitor;

        impl<'de> serde::de::Visitor<'de> for CollectionVisitor {
            type Value = crate::proto::proximadb_v1::Collection;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct Collection")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::Collection, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut id = None;
                let mut config = None;
                let mut stats = None;
                let mut created_at = None;
                let mut updated_at = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Id => {
                            if id.is_some() {
                                return Err(serde::de::Error::duplicate_field("id"));
                            }
                            id = Some(map.next_value()?);
                        }
                        Field::Config => {
                            if config.is_some() {
                                return Err(serde::de::Error::duplicate_field("config"));
                            }
                            config = Some(map.next_value()?);
                        }
                        Field::Stats => {
                            if stats.is_some() {
                                return Err(serde::de::Error::duplicate_field("stats"));
                            }
                            stats = Some(map.next_value()?);
                        }
                        Field::CreatedAt => {
                            if created_at.is_some() {
                                return Err(serde::de::Error::duplicate_field("created_at"));
                            }
                            created_at = Some(map.next_value()?);
                        }
                        Field::UpdatedAt => {
                            if updated_at.is_some() {
                                return Err(serde::de::Error::duplicate_field("updated_at"));
                            }
                            updated_at = Some(map.next_value()?);
                        }
                    }
                }

                let id = id.ok_or_else(|| serde::de::Error::missing_field("id"))?;
                let created_at = created_at.ok_or_else(|| serde::de::Error::missing_field("created_at"))?;
                let updated_at = updated_at.ok_or_else(|| serde::de::Error::missing_field("updated_at"))?;

                Ok(crate::proto::proximadb_v1::Collection {
                    id,
                    config,
                    stats,
                    created_at,
                    updated_at,
                })
            }
        }

        deserializer.deserialize_struct("Collection", &["id", "config", "stats", "created_at", "updated_at"], CollectionVisitor)
    }
}

// Custom serde for CollectionConfig
impl Serialize for crate::proto::proximadb_v1::CollectionConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("CollectionConfig", 5)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("dimension", &self.dimension)?;
        state.serialize_field("distance_metric", &self.distance_metric)?;
        state.serialize_field("storage_engine", &self.storage_engine)?;
        state.serialize_field("tags", &self.tags)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::CollectionConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Name,
            Dimension,
            DistanceMetric,
            StorageEngine,
            Tags,
        }

        struct CollectionConfigVisitor;

        impl<'de> serde::de::Visitor<'de> for CollectionConfigVisitor {
            type Value = crate::proto::proximadb_v1::CollectionConfig;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct CollectionConfig")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::CollectionConfig, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut name = None;
                let mut dimension = None;
                let mut distance_metric = None;
                let mut storage_engine = None;
                let mut tags = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Name => {
                            if name.is_some() {
                                return Err(serde::de::Error::duplicate_field("name"));
                            }
                            name = Some(map.next_value()?);
                        }
                        Field::Dimension => {
                            if dimension.is_some() {
                                return Err(serde::de::Error::duplicate_field("dimension"));
                            }
                            dimension = Some(map.next_value()?);
                        }
                        Field::DistanceMetric => {
                            if distance_metric.is_some() {
                                return Err(serde::de::Error::duplicate_field("distance_metric"));
                            }
                            distance_metric = Some(map.next_value()?);
                        }
                        Field::StorageEngine => {
                            if storage_engine.is_some() {
                                return Err(serde::de::Error::duplicate_field("storage_engine"));
                            }
                            storage_engine = Some(map.next_value()?);
                        }
                        Field::Tags => {
                            if tags.is_some() {
                                return Err(serde::de::Error::duplicate_field("tags"));
                            }
                            tags = Some(map.next_value()?);
                        }
                    }
                }

                let name = name.ok_or_else(|| serde::de::Error::missing_field("name"))?;
                let dimension = dimension.ok_or_else(|| serde::de::Error::missing_field("dimension"))?;
                let distance_metric = distance_metric.ok_or_else(|| serde::de::Error::missing_field("distance_metric"))?;
                let storage_engine = storage_engine.ok_or_else(|| serde::de::Error::missing_field("storage_engine"))?;
                let tags = tags.unwrap_or_default();

                Ok(crate::proto::proximadb_v1::CollectionConfig {
                    name,
                    dimension,
                    distance_metric,
                    storage_engine,
                    tags,
                })
            }
        }

        deserializer.deserialize_struct("CollectionConfig", &["name", "dimension", "distance_metric", "storage_engine", "tags"], CollectionConfigVisitor)
    }
}

// Custom serde for CollectionStats
impl Serialize for crate::proto::proximadb_v1::CollectionStats {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("CollectionStats", 3)?;
        state.serialize_field("vector_count", &self.vector_count)?;
        state.serialize_field("index_size_bytes", &self.index_size_bytes)?;
        state.serialize_field("data_size_bytes", &self.data_size_bytes)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::CollectionStats {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            VectorCount,
            IndexSizeBytes,
            DataSizeBytes,
        }

        struct CollectionStatsVisitor;

        impl<'de> serde::de::Visitor<'de> for CollectionStatsVisitor {
            type Value = crate::proto::proximadb_v1::CollectionStats;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct CollectionStats")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::CollectionStats, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut vector_count = None;
                let mut index_size_bytes = None;
                let mut data_size_bytes = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::VectorCount => {
                            if vector_count.is_some() {
                                return Err(serde::de::Error::duplicate_field("vector_count"));
                            }
                            vector_count = Some(map.next_value()?);
                        }
                        Field::IndexSizeBytes => {
                            if index_size_bytes.is_some() {
                                return Err(serde::de::Error::duplicate_field("index_size_bytes"));
                            }
                            index_size_bytes = Some(map.next_value()?);
                        }
                        Field::DataSizeBytes => {
                            if data_size_bytes.is_some() {
                                return Err(serde::de::Error::duplicate_field("data_size_bytes"));
                            }
                            data_size_bytes = Some(map.next_value()?);
                        }
                    }
                }

                let vector_count = vector_count.ok_or_else(|| serde::de::Error::missing_field("vector_count"))?;
                let index_size_bytes = index_size_bytes.ok_or_else(|| serde::de::Error::missing_field("index_size_bytes"))?;
                let data_size_bytes = data_size_bytes.ok_or_else(|| serde::de::Error::missing_field("data_size_bytes"))?;

                Ok(crate::proto::proximadb_v1::CollectionStats {
                    vector_count,
                    index_size_bytes,
                    data_size_bytes,
                })
            }
        }

        deserializer.deserialize_struct("CollectionStats", &["vector_count", "index_size_bytes", "data_size_bytes"], CollectionStatsVisitor)
    }
}

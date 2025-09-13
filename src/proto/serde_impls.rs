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
                    quantized_vector: Vec::new(),
                    source: Some(String::new()),
                    version: Some(0),
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
                    storage_assignment: None, // TODO: Implement storage assignment
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
                    auto_index_selection: false, // TODO: Implement auto index selection
                    description: String::new(), // TODO: Implement description
                    embedding_models: Vec::new(), // TODO: Implement embedding models
                    enable_compression: false, // TODO: Implement compression
                    index_config: None, // TODO: Implement index config
                    retention_policy: None, // TODO: Implement retention policy
                    replication_factor: 1, // TODO: Implement replication factor
                    sharding_config: None, // TODO: Implement sharding config
                    access_control: None, // TODO: Implement access control
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

// Custom serde for Entity
impl Serialize for crate::proto::proximadb_v1::Entity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Entity", 6)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("embeddings", &self.embeddings)?;
        state.serialize_field("typed_metadata", &self.typed_metadata)?;
        state.serialize_field("flexible_metadata", &self.flexible_metadata)?;
        state.serialize_field("provenance", &self.provenance)?;
        state.serialize_field("relations", &self.relations)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::Entity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Id,
            Embeddings,
            TypedMetadata,
            FlexibleMetadata,
            Provenance,
            Relations,
        }

        struct EntityVisitor;

        impl<'de> serde::de::Visitor<'de> for EntityVisitor {
            type Value = crate::proto::proximadb_v1::Entity;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct Entity")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::Entity, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut id = None;
                let mut embeddings = None;
                let mut typed_metadata = None;
                let mut flexible_metadata = None;
                let mut provenance = None;
                let mut relations = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Id => {
                            if id.is_some() {
                                return Err(serde::de::Error::duplicate_field("id"));
                            }
                            id = Some(map.next_value()?);
                        }
                        Field::Embeddings => {
                            if embeddings.is_some() {
                                return Err(serde::de::Error::duplicate_field("embeddings"));
                            }
                            embeddings = Some(map.next_value()?);
                        }
                        Field::TypedMetadata => {
                            if typed_metadata.is_some() {
                                return Err(serde::de::Error::duplicate_field("typed_metadata"));
                            }
                            typed_metadata = Some(map.next_value()?);
                        }
                        Field::FlexibleMetadata => {
                            if flexible_metadata.is_some() {
                                return Err(serde::de::Error::duplicate_field("flexible_metadata"));
                            }
                            flexible_metadata = Some(map.next_value()?);
                        }
                        Field::Provenance => {
                            if provenance.is_some() {
                                return Err(serde::de::Error::duplicate_field("provenance"));
                            }
                            provenance = Some(map.next_value()?);
                        }
                        Field::Relations => {
                            if relations.is_some() {
                                return Err(serde::de::Error::duplicate_field("relations"));
                            }
                            relations = Some(map.next_value()?);
                        }
                    }
                }

                let id = id.ok_or_else(|| serde::de::Error::missing_field("id"))?;
                let embeddings = embeddings.unwrap_or_default();
                let flexible_metadata = flexible_metadata.unwrap_or_default();
                let relations = relations.unwrap_or_default();

                Ok(crate::proto::proximadb_v1::Entity {
                    id,
                    embeddings,
                    typed_metadata,
                    flexible_metadata,
                    provenance,
                    relations,
                })
            }
        }

        deserializer.deserialize_struct("Entity", &["id", "embeddings", "typed_metadata", "flexible_metadata", "provenance", "relations"], EntityVisitor)
    }
}

// Custom serde for EntityResult
impl Serialize for crate::proto::proximadb_v1::EntityResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("EntityResult", 3)?;
        state.serialize_field("entity", &self.entity)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("debug_info", &self.debug_info)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::EntityResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Entity,
            Score,
            DebugInfo,
        }

        struct EntityResultVisitor;

        impl<'de> serde::de::Visitor<'de> for EntityResultVisitor {
            type Value = crate::proto::proximadb_v1::EntityResult;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct EntityResult")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::EntityResult, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut entity = None;
                let mut score = None;
                let mut debug_info = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Entity => {
                            if entity.is_some() {
                                return Err(serde::de::Error::duplicate_field("entity"));
                            }
                            entity = Some(map.next_value()?);
                        }
                        Field::Score => {
                            if score.is_some() {
                                return Err(serde::de::Error::duplicate_field("score"));
                            }
                            score = Some(map.next_value()?);
                        }
                        Field::DebugInfo => {
                            if debug_info.is_some() {
                                return Err(serde::de::Error::duplicate_field("debug_info"));
                            }
                            debug_info = Some(map.next_value()?);
                        }
                    }
                }

                let score = score.ok_or_else(|| serde::de::Error::missing_field("score"))?;
                let debug_info = debug_info.unwrap_or_default();

                Ok(crate::proto::proximadb_v1::EntityResult {
                    entity,
                    score,
                    debug_info,
                })
            }
        }

        deserializer.deserialize_struct("EntityResult", &["entity", "score", "debug_info"], EntityResultVisitor)
    }
}

// Custom serde for VectorOperationResponse
impl Serialize for crate::proto::proximadb_v1::VectorOperationResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("VectorOperationResponse", 5)?;
        state.serialize_field("success", &self.success)?;
        state.serialize_field("operation", &self.operation)?;
        state.serialize_field("metrics", &self.metrics)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("warnings", &self.warnings)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::VectorOperationResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Success,
            Operation,
            Metrics,
            Results,
            Warnings,
        }

        struct VectorOperationResponseVisitor;

        impl<'de> serde::de::Visitor<'de> for VectorOperationResponseVisitor {
            type Value = crate::proto::proximadb_v1::VectorOperationResponse;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct VectorOperationResponse")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::VectorOperationResponse, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut success = None;
                let mut operation = None;
                let mut metrics = None;
                let mut results = None;
                let mut warnings = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Success => {
                            if success.is_some() {
                                return Err(serde::de::Error::duplicate_field("success"));
                            }
                            success = Some(map.next_value()?);
                        }
                        Field::Operation => {
                            if operation.is_some() {
                                return Err(serde::de::Error::duplicate_field("operation"));
                            }
                            operation = Some(map.next_value()?);
                        }
                        Field::Metrics => {
                            if metrics.is_some() {
                                return Err(serde::de::Error::duplicate_field("metrics"));
                            }
                            metrics = Some(map.next_value()?);
                        }
                        Field::Results => {
                            if results.is_some() {
                                return Err(serde::de::Error::duplicate_field("results"));
                            }
                            results = Some(map.next_value()?);
                        }
                        Field::Warnings => {
                            if warnings.is_some() {
                                return Err(serde::de::Error::duplicate_field("warnings"));
                            }
                            warnings = Some(map.next_value()?);
                        }
                    }
                }

                let success = success.ok_or_else(|| serde::de::Error::missing_field("success"))?;
                let operation = operation.ok_or_else(|| serde::de::Error::missing_field("operation"))?;
                let warnings = warnings.unwrap_or_default();

                Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                    success,
                    operation,
                    metrics,
                    results,
                    warnings,
                })
            }
        }

        deserializer.deserialize_struct("VectorOperationResponse", &["success", "operation", "metrics", "results", "warnings"], VectorOperationResponseVisitor)
    }
}

// Custom serde for CollectionResponse
impl Serialize for crate::proto::proximadb_v1::CollectionResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("CollectionResponse", 5)?;
        state.serialize_field("success", &self.success)?;
        state.serialize_field("collection", &self.collection)?;
        state.serialize_field("collections", &self.collections)?;
        state.serialize_field("error_message", &self.error_message)?;
        state.serialize_field("error_code", &self.error_code)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::CollectionResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Success,
            Collection,
            Collections,
            ErrorMessage,
            ErrorCode,
        }

        struct CollectionResponseVisitor;

        impl<'de> serde::de::Visitor<'de> for CollectionResponseVisitor {
            type Value = crate::proto::proximadb_v1::CollectionResponse;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct CollectionResponse")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::CollectionResponse, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut success = None;
                let mut collection = None;
                let mut collections = None;
                let mut error_message = None;
                let mut error_code = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Success => {
                            if success.is_some() {
                                return Err(serde::de::Error::duplicate_field("success"));
                            }
                            success = Some(map.next_value()?);
                        }
                        Field::Collection => {
                            if collection.is_some() {
                                return Err(serde::de::Error::duplicate_field("collection"));
                            }
                            collection = Some(map.next_value()?);
                        }
                        Field::Collections => {
                            if collections.is_some() {
                                return Err(serde::de::Error::duplicate_field("collections"));
                            }
                            collections = Some(map.next_value()?);
                        }
                        Field::ErrorMessage => {
                            if error_message.is_some() {
                                return Err(serde::de::Error::duplicate_field("error_message"));
                            }
                            error_message = Some(map.next_value()?);
                        }
                        Field::ErrorCode => {
                            if error_code.is_some() {
                                return Err(serde::de::Error::duplicate_field("error_code"));
                            }
                            error_code = Some(map.next_value()?);
                        }
                    }
                }

                let success = success.ok_or_else(|| serde::de::Error::missing_field("success"))?;
                let collections = collections.unwrap_or_default();

                Ok(crate::proto::proximadb_v1::CollectionResponse {
                    success,
                    collection,
                    collections,
                    error_message,
                    error_code,
                })
            }
        }

        deserializer.deserialize_struct("CollectionResponse", &["success", "collection", "collections", "error_message", "error_code"], CollectionResponseVisitor)
    }
}

// Implement Serialize for sql_value::Value enum
impl Serialize for crate::proto::proximadb_v1::sql_value::Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use crate::proto::proximadb_v1::sql_value::Value;
        match self {
            Value::StringValue(v) => ("string_value", v).serialize(serializer),
            Value::NumberValue(v) => ("number_value", v).serialize(serializer),
            Value::BoolValue(v) => ("bool_value", v).serialize(serializer),
            Value::Int64Value(v) => ("int64_value", v).serialize(serializer),
            Value::BytesValue(v) => ("bytes_value", base64_encode(v)).serialize(serializer),
            Value::NullValue(v) => ("null_value", v).serialize(serializer),
            Value::ArrayValue(v) => ("array_value", v).serialize(serializer),
            Value::ObjectValue(v) => ("object_value", v).serialize(serializer),
        }
    }
}

// Implement Serialize for filter_clause::Value enum
impl Serialize for crate::proto::proximadb_v1::filter_clause::Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use crate::proto::proximadb_v1::filter_clause::Value;
        match self {
            Value::StringValue(v) => ("string_value", v).serialize(serializer),
            Value::IntValue(v) => ("int_value", v).serialize(serializer),
            Value::DoubleValue(v) => ("double_value", v).serialize(serializer),
            Value::BoolValue(v) => ("bool_value", v).serialize(serializer),
        }
    }
}

// Add more basic proto implementations
impl Serialize for crate::proto::proximadb_v1::TypedMetadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("TypedMetadata", 2)?;
        state.serialize_field("metadata_type", &self.metadata_type)?;
        state.serialize_field("properties", &self.properties)?;
        state.end()
    }
}

impl Serialize for crate::proto::proximadb_v1::Provenance {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Provenance", 4)?;
        state.serialize_field("source_system", &self.source_system)?;
        state.serialize_field("data_lineage", &self.data_lineage)?;
        state.serialize_field("created_at", &self.created_at)?;
        state.serialize_field("confidence_score", &self.confidence_score)?;
        state.end()
    }
}

// Custom serde for MetadataFilter
impl Serialize for crate::proto::proximadb_v1::MetadataFilter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("MetadataFilter", 2)?;
        state.serialize_field("clauses", &self.clauses)?;
        state.serialize_field("op", &self.op)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::MetadataFilter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Clauses,
            Op,
        }

        struct MetadataFilterVisitor;

        impl<'de> serde::de::Visitor<'de> for MetadataFilterVisitor {
            type Value = crate::proto::proximadb_v1::MetadataFilter;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct MetadataFilter")
            }

            fn visit_map<V>(self, mut map: V) -> Result<crate::proto::proximadb_v1::MetadataFilter, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut clauses = None;
                let mut op = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Clauses => {
                            if clauses.is_some() {
                                return Err(serde::de::Error::duplicate_field("clauses"));
                            }
                            clauses = Some(map.next_value()?);
                        }
                        Field::Op => {
                            if op.is_some() {
                                return Err(serde::de::Error::duplicate_field("op"));
                            }
                            op = Some(map.next_value()?);
                        }
                    }
                }

                let clauses = clauses.unwrap_or_default();
                let op = op.ok_or_else(|| serde::de::Error::missing_field("op"))?;

                Ok(crate::proto::proximadb_v1::MetadataFilter { clauses, op })
            }
        }

        deserializer.deserialize_struct("MetadataFilter", &["clauses", "op"], MetadataFilterVisitor)
    }
}


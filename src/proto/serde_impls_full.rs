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
        let mut state = serializer.serialize_struct("Collection", 6)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("config", &self.config)?;
        state.serialize_field("stats", &self.stats)?;
        state.serialize_field("created_at", &self.created_at)?;
        state.serialize_field("updated_at", &self.updated_at)?;
        state.serialize_field("storage_assignment", &self.storage_assignment)?;
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
            StorageAssignment,
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
                let mut storage_assignment = None;

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
                        Field::StorageAssignment => {
                            if storage_assignment.is_some() {
                                return Err(serde::de::Error::duplicate_field("storage_assignment"));
                            }
                            storage_assignment = Some(map.next_value()?);
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
                    storage_assignment,
                })
            }
        }

        deserializer.deserialize_struct("Collection", &["id", "config", "stats", "created_at", "updated_at", "storage_assignment"], CollectionVisitor)
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
            Description,
            FilterableColumns,
            AutoIndexSelection,
            Owner,
            PrimaryIndex,
            EmbeddingModels,
            #[serde(other)]
            Unknown,
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
                let mut description: Option<String> = None;
                let mut filterable_columns: Option<Vec<String>> = None;
                let mut auto_index_selection: Option<bool> = None;
                let mut owner: Option<String> = None;
                let mut primary_index: Option<String> = None;
                let mut embedding_models: Option<Vec<String>> = None;

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
                            let value: i32 = map.next_value()?;
                            tracing::info!("🔍 Deserializing distance_metric: {}", value);
                            distance_metric = Some(value);
                        }
                        Field::StorageEngine => {
                            if storage_engine.is_some() {
                                return Err(serde::de::Error::duplicate_field("storage_engine"));
                            }
                            let value: i32 = map.next_value()?;
                            tracing::info!("🔍 Deserializing storage_engine: {}", value);
                            storage_engine = Some(value);
                        }
                        Field::Tags => {
                            if tags.is_some() {
                                return Err(serde::de::Error::duplicate_field("tags"));
                            }
                            tags = Some(map.next_value()?);
                        }
                        Field::Description => { description = Some(map.next_value()?); }
                        Field::FilterableColumns => { filterable_columns = Some(map.next_value()?); }
                        Field::AutoIndexSelection => { auto_index_selection = Some(map.next_value()?); }
                        Field::Owner => { owner = Some(map.next_value()?); }
                        Field::PrimaryIndex => { primary_index = Some(map.next_value()?); }
                        Field::EmbeddingModels => { embedding_models = Some(map.next_value()?); }
                        Field::Unknown => { let _: serde::de::IgnoredAny = map.next_value()?; }
                    }
                }

                let name = name.ok_or_else(|| serde::de::Error::missing_field("name"))?;
                let dimension = dimension.ok_or_else(|| serde::de::Error::missing_field("dimension"))?;
                // distance_metric and storage_engine are optional in proto, use None if not provided
                let distance_metric = distance_metric;
                let storage_engine = storage_engine;
                let tags = tags.unwrap_or_default();

                Ok(crate::proto::proximadb_v1::CollectionConfig {
                    name,
                    dimension,
                    distance_metric,
                    storage_engine,
                    tags,
                    description,
                    filterable_columns: filterable_columns.unwrap_or_default(),
                    index_configs: Vec::new(), // Complex nested type — deferred to index config serde
                    quantization: None, // Complex nested type — deferred to quantization serde
                    storage_config: None, // Complex nested type — deferred to storage config serde
                    primary_index: primary_index.unwrap_or_default(),
                    auto_index_selection: auto_index_selection.unwrap_or(false),
                    owner,
                    embedding_models: embedding_models.unwrap_or_default(),
                })
            }
        }

        deserializer.deserialize_struct("CollectionConfig", &[
            "name", "dimension", "distance_metric", "storage_engine", "tags",
            "description", "filterable_columns", "auto_index_selection", "owner",
            "primary_index", "embedding_models",
        ], CollectionConfigVisitor)
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
            Id, Embeddings, TypedMetadata, FlexibleMetadata, Provenance, Relations,
            CollectionId, Temporal,
            #[serde(other)] Unknown,
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
                let mut collection_id: Option<String> = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Id => { id = Some(map.next_value()?); }
                        Field::Embeddings => { embeddings = Some(map.next_value()?); }
                        Field::TypedMetadata => { typed_metadata = Some(map.next_value()?); }
                        Field::FlexibleMetadata => { flexible_metadata = Some(map.next_value()?); }
                        Field::Provenance => { provenance = Some(map.next_value()?); }
                        Field::Relations => { relations = Some(map.next_value()?); }
                        Field::CollectionId => { collection_id = Some(map.next_value()?); }
                        Field::Temporal => { let _: serde::de::IgnoredAny = map.next_value()?; }
                        Field::Unknown => { let _: serde::de::IgnoredAny = map.next_value()?; }
                    }
                }

                Ok(crate::proto::proximadb_v1::Entity {
                    id: id.ok_or_else(|| serde::de::Error::missing_field("id"))?,
                    embeddings: embeddings.unwrap_or_default(),
                    typed_metadata,
                    flexible_metadata: flexible_metadata.unwrap_or_default(),
                    provenance,
                    relations: relations.unwrap_or_default(),
                    collection_id: collection_id.unwrap_or_default(),
                    temporal: None, // Complex nested type; deserialized by temporal serde
                })
            }
        }

        deserializer.deserialize_struct("Entity", &[
            "id", "embeddings", "typed_metadata", "flexible_metadata",
            "provenance", "relations", "collection_id", "temporal",
        ], EntityVisitor)
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
        // warnings field doesn't exist in VectorOperationResponse - removing this line
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
            VectorIds,
            ErrorMessage,
            ErrorCode,
            #[serde(other)]
            Unknown,
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
                let mut warnings: Option<Vec<String>> = None;
                let mut vector_ids: Option<Vec<String>> = None;
                let mut error_message: Option<String> = None;
                let mut error_code: Option<i32> = None;

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
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                        Field::VectorIds => { vector_ids = Some(map.next_value()?); }
                        Field::ErrorMessage => { error_message = Some(map.next_value()?); }
                        Field::ErrorCode => { error_code = Some(map.next_value()?); }
                        Field::Unknown => { let _: serde::de::IgnoredAny = map.next_value()?; }
                    }
                }

                let success = success.ok_or_else(|| serde::de::Error::missing_field("success"))?;
                let operation = operation.ok_or_else(|| serde::de::Error::missing_field("operation"))?;

                Ok(crate::proto::proximadb_v1::VectorOperationResponse {
                    success,
                    operation,
                    metrics,
                    results,
                    vector_ids: vector_ids.unwrap_or_default(),
                    error_message,
                    error_code,
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
            Success, Collection, Collections, ErrorMessage, ErrorCode,
            Operation, AffectedCount, TotalCount, Metadata, ProcessingTimeUs,
            #[serde(other)] Unknown,
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
                let mut operation: Option<i32> = None;
                let mut affected_count: Option<i64> = None;
                let mut total_count: Option<i64> = None;
                let mut metadata: Option<std::collections::HashMap<String, String>> = None;
                let mut processing_time_us: Option<i64> = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Success => { success = Some(map.next_value()?); }
                        Field::Collection => { collection = Some(map.next_value()?); }
                        Field::Collections => { collections = Some(map.next_value()?); }
                        Field::ErrorMessage => { error_message = Some(map.next_value()?); }
                        Field::ErrorCode => { error_code = Some(map.next_value()?); }
                        Field::Operation => { operation = Some(map.next_value()?); }
                        Field::AffectedCount => { affected_count = Some(map.next_value()?); }
                        Field::TotalCount => { total_count = Some(map.next_value()?); }
                        Field::Metadata => { metadata = Some(map.next_value()?); }
                        Field::ProcessingTimeUs => { processing_time_us = Some(map.next_value()?); }
                        Field::Unknown => { let _: serde::de::IgnoredAny = map.next_value()?; }
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
                    operation: operation.unwrap_or(0),
                    affected_count: affected_count.unwrap_or(0),
                    total_count: total_count.unwrap_or(0),
                    metadata: metadata.unwrap_or_default(),
                    processing_time_us: processing_time_us.unwrap_or(0),
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
        let mut state = serializer.serialize_struct("TypedMetadata", 1)?;
        // Serialize fields as HashMap with TypedField serde support
        let mut fields_map: std::collections::HashMap<String, TypedFieldDef> = std::collections::HashMap::new();
        for (k, v) in &self.fields {
            fields_map.insert(k.clone(), TypedFieldDef {
                indexed: v.indexed,
                filterable: v.filterable,
                value: v.value.as_ref().map(|val| match val {
                    crate::proto::proximadb_v1::typed_field::Value::StringValue(s) => typed_field_def::Value::StringValue(s.clone()),
                    crate::proto::proximadb_v1::typed_field::Value::IntValue(i) => typed_field_def::Value::IntValue(*i),
                    crate::proto::proximadb_v1::typed_field::Value::DoubleValue(f) => typed_field_def::Value::FloatValue(*f),
                    crate::proto::proximadb_v1::typed_field::Value::BoolValue(b) => typed_field_def::Value::BoolValue(*b),
                    crate::proto::proximadb_v1::typed_field::Value::StringArray(arr) => typed_field_def::Value::ListValue(arr.values.iter().map(|s| {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s.clone())),
                        }
                    }).collect()),
                    crate::proto::proximadb_v1::typed_field::Value::TimestampValueMs(ts) => typed_field_def::Value::IntValue(*ts),
                }),
            });
        }
        state.serialize_field("fields", &fields_map)?;
        state.end()
    }
}

impl Serialize for crate::proto::proximadb_v1::Provenance {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Provenance", 6)?;
        state.serialize_field("source_id", &self.source_id)?;
        state.serialize_field("chunk_id", &self.chunk_id)?;
        state.serialize_field("chunk_position", &self.chunk_position)?;
        state.serialize_field("extraction_method", &self.extraction_method)?;
        state.serialize_field("extracted_at_ms", &self.extracted_at_ms)?;
        state.serialize_field("metadata", &self.metadata)?;
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
                            // Deserialize as Vec<FilterClauseDef> and convert
                            let clause_defs: Vec<FilterClauseDef> = map.next_value()?;
                            clauses = Some(clause_defs.into_iter().map(|def| crate::proto::proximadb_v1::FilterClause {
                                field: def.field,
                                op: def.op,
                                value: def.value.map(|val| match val {
                                    filter_clause_def::Value::StringValue(s) => crate::proto::proximadb_v1::filter_clause::Value::StringValue(s),
                                    filter_clause_def::Value::IntValue(i) => crate::proto::proximadb_v1::filter_clause::Value::IntValue(i),
                                    filter_clause_def::Value::FloatValue(f) => crate::proto::proximadb_v1::filter_clause::Value::DoubleValue(f),
                                    filter_clause_def::Value::BoolValue(b) => crate::proto::proximadb_v1::filter_clause::Value::BoolValue(b),
                                }),
                            }).collect());
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

// Add missing Serialize/Deserialize implementations
// Note: Serialize and Deserialize are already imported at the top of the file

// Add Serialize/Deserialize for Relation
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::proto::proximadb_v1::Relation")]
struct RelationDef {
    source_entity_id: String,
    target_entity_id: String,
    relation_type: String,
    weight: f32,
    created_at_ms: i64,
    properties: std::collections::HashMap<String, String>,
}

impl Serialize for crate::proto::proximadb_v1::Relation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        RelationDef::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::Relation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        RelationDef::deserialize(deserializer)
    }
}

// Add Serialize/Deserialize for TypedMetadata
impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::TypedMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct TypedMetadataHelper {
            fields: std::collections::HashMap<String, TypedFieldDef>,
        }

        let helper = TypedMetadataHelper::deserialize(deserializer)?;
        let mut converted_fields = std::collections::HashMap::new();
        for (k, def) in helper.fields {
            converted_fields.insert(k, crate::proto::proximadb_v1::TypedField {
                indexed: def.indexed,
                filterable: def.filterable,
                value: def.value.map(|val| match val {
                    typed_field_def::Value::StringValue(s) => crate::proto::proximadb_v1::typed_field::Value::StringValue(s),
                    typed_field_def::Value::IntValue(i) => crate::proto::proximadb_v1::typed_field::Value::IntValue(i),
                    typed_field_def::Value::FloatValue(f) => crate::proto::proximadb_v1::typed_field::Value::DoubleValue(f),
                    typed_field_def::Value::BoolValue(b) => crate::proto::proximadb_v1::typed_field::Value::BoolValue(b),
                    typed_field_def::Value::ListValue(l) => {
                        let string_values: Vec<String> = l.iter().filter_map(|sql_val| {
                            match &sql_val.value {
                                Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => Some(s.clone()),
                                _ => None,
                            }
                        }).collect();
                        crate::proto::proximadb_v1::typed_field::Value::StringArray(crate::proto::proximadb_v1::StringArray { values: string_values })
                    },
                    typed_field_def::Value::MapValue(_) => {
                        // For now, convert maps to timestamp (this is a temporary fix)
                        crate::proto::proximadb_v1::typed_field::Value::TimestampValueMs(0)
                    },
                }),
            });
        }
        Ok(crate::proto::proximadb_v1::TypedMetadata {
            fields: converted_fields,
        })
    }
}

// Add Serialize/Deserialize for Provenance
impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::Provenance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ProvenanceHelper {
            source_id: String,
            chunk_id: String,
            chunk_position: u32,
            extraction_method: String,
            extracted_at_ms: i64,
            metadata: std::collections::HashMap<String, String>,
        }

        let helper = ProvenanceHelper::deserialize(deserializer)?;
        Ok(crate::proto::proximadb_v1::Provenance {
            source_id: helper.source_id,
            chunk_id: helper.chunk_id,
            chunk_position: helper.chunk_position,
            extraction_method: helper.extraction_method,
            extracted_at_ms: helper.extracted_at_ms,
            metadata: helper.metadata,
        })
    }
}

// Add Serialize/Deserialize for OperationMetrics
#[derive(Serialize, Deserialize)]
#[serde(remote = "crate::proto::proximadb_v1::OperationMetrics")]
struct OperationMetricsDef {
    #[serde(default)]
    total_processed: i64,
    #[serde(default)]
    successful_count: i64,
    #[serde(default)]
    failed_count: i64,
    #[serde(default)]
    updated_count: i64,
    #[serde(default)]
    processing_time_us: i64,
    #[serde(default)]
    wal_write_time_us: i64,
    #[serde(default)]
    index_update_time_us: i64,
}

impl Serialize for crate::proto::proximadb_v1::OperationMetrics {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        OperationMetricsDef::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::OperationMetrics {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        OperationMetricsDef::deserialize(deserializer)
    }
}

// Helper struct for SearchResult serialization (not using remote)
#[derive(Serialize, Deserialize)]
struct SearchResultHelper {
    #[serde(default)]
    results: Vec<SearchVectorRecordDef>,
    #[serde(default)]
    total_found: i64,
    #[serde(default)]
    collection_id: Option<String>,
}

// SearchVectorRecord implementations
impl Serialize for crate::proto::proximadb_v1::SearchVectorRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        SearchVectorRecordDef {
            id: self.id.clone(),
            score: self.score,
            vector: self.vector.clone(),
            metadata: self.metadata.clone(),
            version: self.version,
            similarity: self.similarity,
            timestamp: self.timestamp,
            source: self.source.clone(),
            expanded_context: self.expanded_context.clone(),
            semantic_similarity: self.semantic_similarity,
            quantization_info: self.quantization_info.clone(),
            engine_stats: self.engine_stats.clone(),
            index_path: self.index_path.clone(),
        }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::SearchVectorRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let def = SearchVectorRecordDef::deserialize(deserializer)?;
        Ok(crate::proto::proximadb_v1::SearchVectorRecord {
            id: def.id,
            score: def.score,
            vector: def.vector,
            metadata: def.metadata,
            version: def.version,
            similarity: def.similarity,
            timestamp: def.timestamp,
            source: def.source,
            expanded_context: def.expanded_context,
            semantic_similarity: def.semantic_similarity,
            quantization_info: def.quantization_info,
            engine_stats: def.engine_stats,
            index_path: def.index_path,
        })
    }
}

impl Serialize for crate::proto::proximadb_v1::SearchResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        SearchResultHelper {
            results: self.results.iter().map(|r| SearchVectorRecordDef {
                id: r.id.clone(),
                score: r.score,
                vector: r.vector.clone(),
                metadata: r.metadata.clone(),
                version: r.version,
                similarity: r.similarity,
                timestamp: r.timestamp,
                source: r.source.clone(),
                expanded_context: r.expanded_context.clone(),
                semantic_similarity: r.semantic_similarity,
                quantization_info: r.quantization_info.clone(),
                engine_stats: r.engine_stats.clone(),
                index_path: r.index_path.clone(),
            }).collect(),
            total_found: self.total_found,
            collection_id: self.collection_id.clone(),
        }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for crate::proto::proximadb_v1::SearchResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let def = SearchResultHelper::deserialize(deserializer)?;
        Ok(crate::proto::proximadb_v1::SearchResult {
            results: def.results.into_iter().map(|def| crate::proto::proximadb_v1::SearchVectorRecord {
                id: def.id,
                score: def.score,
                vector: def.vector,
                metadata: def.metadata,
                version: def.version,
                similarity: def.similarity,
                timestamp: def.timestamp,
                source: def.source,
                expanded_context: def.expanded_context,
                semantic_similarity: def.semantic_similarity,
                quantization_info: def.quantization_info,
                engine_stats: def.engine_stats,
                index_path: def.index_path,
            }).collect(),
            total_found: def.total_found,
            collection_id: def.collection_id,
        })
    }
}

// TypedField serde support structures
#[derive(Serialize, Deserialize)]
struct TypedFieldDef {
    indexed: bool,
    filterable: bool,
    value: Option<typed_field_def::Value>,
}

pub mod typed_field_def {
    use serde::{Serialize, Deserialize};

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "type", content = "value")]
    pub enum Value {
        #[serde(rename = "string")]
        StringValue(String),
        #[serde(rename = "int")]
        IntValue(i64),
        #[serde(rename = "float")]
        FloatValue(f64),
        #[serde(rename = "bool")]
        BoolValue(bool),
        #[serde(rename = "list")]
        ListValue(Vec<crate::proto::proximadb_v1::SqlValue>),
        #[serde(rename = "map")]
        MapValue(std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue>),
    }
}

// FilterClause serde support structures
#[derive(Serialize, Deserialize)]
struct FilterClauseDef {
    field: String,
    op: i32,
    value: Option<filter_clause_def::Value>,
}

pub mod filter_clause_def {
    use serde::{Serialize, Deserialize};

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "type", content = "value")]
    pub enum Value {
        #[serde(rename = "string")]
        StringValue(String),
        #[serde(rename = "int")]
        IntValue(i64),
        #[serde(rename = "float")]
        FloatValue(f64),
        #[serde(rename = "bool")]
        BoolValue(bool),
    }
}

// SearchVectorRecord serde support structures
#[derive(Serialize, Deserialize)]
struct SearchVectorRecordDef {
    id: String,
    score: f64,
    #[serde(default)]
    vector: Vec<f32>,
    #[serde(default)]
    metadata: std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    version: Option<i64>,
    similarity: Option<f32>,
    timestamp: Option<i64>,
    source: Option<String>,
    #[serde(default)]
    expanded_context: Vec<String>,
    semantic_similarity: Option<f32>,
    quantization_info: Option<String>,
    #[serde(default)]
    engine_stats: std::collections::HashMap<String, String>,
    index_path: Option<String>,
}

// Custom deserialization helper for CollectionConfig to handle prost enums correctly
// Prost generates enums as Option<i32>, but the default Deserialize derive treats them as strings
pub struct CollectionConfigDeserialize;

impl CollectionConfigDeserialize {
    pub fn deserialize<'de, D>(deserializer: D) -> Result<crate::proto::proximadb_v1::CollectionConfig, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct CollectionConfigVisitor;

        impl<'de> Visitor<'de> for CollectionConfigVisitor {
            type Value = crate::proto::proximadb_v1::CollectionConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct CollectionConfig")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                use crate::proto::proximadb_v1::{
                    CollectionConfig, FilterableColumnSpec, IndexConfig,
                    QuantizationConfig, StorageConfig,
                };

                let mut name = None;
                let mut dimension = None;
                let mut distance_metric = None;
                let mut storage_engine = None;
                let mut tags = None;
                let mut description = None;
                let mut filterable_columns = None;
                let mut index_configs = None;
                let mut quantization = None;
                let mut storage_config = None;
                let mut primary_index = None;
                let mut auto_index_selection = None;
                let mut owner = None;
                let mut replication_factor = None;
                let mut enable_cross_region_replication = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        "name" => {
                            name = Some(map.next_value()?);
                        }
                        "dimension" => {
                            dimension = Some(map.next_value()?);
                        }
                        "distance_metric" => {
                            // Handle both integer and string representations
                            if let Some(value) = map.next_value::<Option<serde_json::Value>>()? {
                                distance_metric = Some(if let Some(num) = value.as_i64() {
                                    num as i32
                                } else if let Some(s) = value.as_str() {
                                    // If string, try to parse as integer
                                    s.parse().unwrap_or(1) // Default to COSINE
                                } else {
                                    1 // Default to COSINE
                                });
                            }
                        }
                        "storage_engine" => {
                            // Handle both integer and string representations
                            if let Some(value) = map.next_value::<Option<serde_json::Value>>()? {
                                storage_engine = Some(if let Some(num) = value.as_i64() {
                                    num as i32
                                } else if let Some(s) = value.as_str() {
                                    // If string, try to parse as integer
                                    s.parse().unwrap_or(2) // Default to SST
                                } else {
                                    2 // Default to SST
                                });
                            }
                        }
                        "tags" => {
                            tags = Some(map.next_value().unwrap_or_default());
                        }
                        "description" => {
                            description = map.next_value()?;
                        }
                        "filterable_columns" => {
                            filterable_columns = Some(map.next_value().unwrap_or_default());
                        }
                        "index_configs" => {
                            index_configs = Some(map.next_value().unwrap_or_default());
                        }
                        "quantization" => {
                            quantization = map.next_value()?;
                        }
                        "storage_config" => {
                            storage_config = map.next_value()?;
                        }
                        "primary_index" => {
                            primary_index = map.next_value()?;
                        }
                        "auto_index_selection" => {
                            auto_index_selection = map.next_value()?;
                        }
                        "owner" => {
                            owner = map.next_value()?;
                        }
                        "replication_factor" => {
                            replication_factor = map.next_value()?;
                        }
                        "enable_cross_region_replication" => {
                            enable_cross_region_replication = map.next_value()?;
                        }
                        _ => {
                            // Ignore unknown fields
                            let _ = map.next_value::<serde::de::IgnoredAny>();
                        }
                    }
                }

                let name = name.ok_or_else(|| de::Error::missing_field("name"))?;
                let dimension = dimension.ok_or_else(|| de::Error::missing_field("dimension"))?;

                Ok(CollectionConfig {
                    name,
                    dimension,
                    distance_metric,
                    storage_engine,
                    tags: tags.unwrap_or_default(),
                    description,
                    filterable_columns: filterable_columns.unwrap_or_default(),
                    index_configs: index_configs.unwrap_or_default(),
                    quantization,
                    storage_config,
                    primary_index,
                    auto_index_selection,
                    owner,
                    replication_factor,
                    enable_cross_region_replication,
                })
            }
        }

        deserializer.deserialize_map(CollectionConfigVisitor)
    }
}


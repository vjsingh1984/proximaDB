//! Generic implementations of base traits for common use cases

use super::base_traits::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt::Debug;
use std::marker::PhantomData;

/// Generic configuration wrapper that implements BaseConfig
#[derive(Debug, Clone)]
pub struct GenericConfig<T> {
    pub data: T,
    pub validation_rules: HashMap<String, String>,
    _phantom: PhantomData<T>,
}

impl<T> GenericConfig<T>
where
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
{
    pub fn new(data: T) -> Self {
        Self {
            data,
            validation_rules: HashMap::new(),
            _phantom: PhantomData,
        }
    }

    pub fn with_validation(mut self, rules: HashMap<String, String>) -> Self {
        self.validation_rules = rules;
        self
    }
}

impl<T> BaseConfig for GenericConfig<T>
where
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
{
    fn validate(&self) -> Result<(), String> {
        // Apply validation rules
        for (rule, message) in &self.validation_rules {
            // Simple validation framework - can be extended
            if rule == "required" && message.is_empty() {
                return Err(format!("Validation failed: {}", message));
            }
        }
        Ok(())
    }
}

/// Generic metadata wrapper that implements BaseMetadata
#[derive(Debug, Clone)]
pub struct GenericMetadata<T> {
    pub id: String,
    pub data: T,
    pub version: u64,
    pub timestamp: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub tags: Vec<String>,
    pub properties: HashMap<String, serde_json::Value>,
}

impl<T> GenericMetadata<T>
where
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
{
    pub fn new(id: String, data: T) -> Self {
        let now = Utc::now();
        Self {
            id,
            data,
            version: 1,
            timestamp: now,
            updated_at: now,
            tags: Vec::new(),
            properties: HashMap::new(),
        }
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_properties(mut self, properties: HashMap<String, serde_json::Value>) -> Self {
        self.properties = properties;
        self
    }
}

impl<T> BaseMetadata for GenericMetadata<T>
where
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
{
    fn version(&self) -> u64 {
        self.version
    }

    fn id(&self) -> String {
        self.id.clone()
    }

    fn created_at(&self) -> DateTime<Utc> {
        self.timestamp
    }

    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Generic statistics wrapper that implements BaseStats
#[derive(Debug, Clone)]
pub struct GenericStats<T> {
    pub data: T,
    pub timestamp: DateTime<Utc>,
    pub collection_count: u64,
    pub reset_count: u64,
}

impl<T> GenericStats<T>
where
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
{
    pub fn new(data: T) -> Self {
        Self {
            data,
            timestamp: Utc::now(),
            collection_count: 1,
            reset_count: 0,
        }
    }

    pub fn update_data(&mut self, data: T) {
        self.data = data;
        self.timestamp = Utc::now();
    }
}

impl<T> BaseStats for GenericStats<T>
where
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
{
    fn aggregate(&mut self, other: &Self) {
        self.collection_count += other.collection_count;
        self.timestamp = Utc::now();
    }

    fn reset(&mut self) {
        self.collection_count = 0;
        self.reset_count += 1;
        self.timestamp = Utc::now();
    }

    fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }
}

/// Generic result wrapper that implements BaseResult
#[derive(Debug, Clone)]
pub struct GenericResult<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error_code: Option<String>,
    pub processing_time_us: Option<u64>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl<T> GenericResult<T>
where
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
{
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            // error_message removed -  None,
            error_code: None,
            processing_time_us: None,
            metadata: HashMap::new(),
        }
    }

    pub fn error() -> Self {
        Self {
            success: false,
            data: None,
            error_code: None,
            processing_time_us: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_processing_time(mut self, time_us: u64) -> Self {
        self.processing_time_us = Some(time_us);
        self
    }

    pub fn with_error_code(mut self, code: String) -> Self {
        self.error_code = Some(code);
        self
    }
}

impl<T> BaseResult<T> for GenericResult<T>
where
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync,
{
    fn is_success(&self) -> bool {
        self.success
    }

    fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }

    fn error(&self) -> Option<&str> {
        self.error_code.as_deref()
    }

    fn processing_time_us(&self) -> Option<u64> {
        self.processing_time_us
    }
}

// Manual Serialize/Deserialize implementations for generic types
impl<T> Serialize for GenericConfig<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("GenericConfig", 2)?;
        state.serialize_field("data", &self.data)?;
        state.serialize_field("validation_rules", &self.validation_rules)?;
        state.end()
    }
}

impl<'de, T> Deserialize<'de> for GenericConfig<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct GenericConfigVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for GenericConfigVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = GenericConfig<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct GenericConfig")
            }

            fn visit_map<V>(self, mut map: V) -> Result<GenericConfig<T>, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut data = None;
                let mut validation_rules = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "data" => {
                            if data.is_some() {
                                return Err(de::Error::duplicate_field("data"));
                            }
                            data = Some(map.next_value()?);
                        }
                        "validation_rules" => {
                            if validation_rules.is_some() {
                                return Err(de::Error::duplicate_field("validation_rules"));
                            }
                            validation_rules = Some(map.next_value()?);
                        }
                        _ => {
                            let _ = map.next_value::<serde_json::Value>()?;
                        }
                    }
                }

                let data = data.ok_or_else(|| de::Error::missing_field("data"))?;
                let validation_rules = validation_rules.unwrap_or_else(HashMap::new);

                Ok(GenericConfig {
                    data,
                    validation_rules,
                    _phantom: PhantomData,
                })
            }
        }

        deserializer.deserialize_map(GenericConfigVisitor(PhantomData))
    }
}

impl<T> Serialize for GenericMetadata<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("GenericMetadata", 7)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("data", &self.data)?;
        state.serialize_field("version", &self.version)?;
        state.serialize_field("timestamp", &self.timestamp)?;
        state.serialize_field("updated_at", &self.updated_at)?;
        state.serialize_field("tags", &self.tags)?;
        state.serialize_field("properties", &self.properties)?;
        state.end()
    }
}

impl<'de, T> Deserialize<'de> for GenericMetadata<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Id,
            Data,
            Version,
            Timestamp,
            UpdatedAt,
            Tags,
            Properties,
        }

        struct GenericMetadataVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for GenericMetadataVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = GenericMetadata<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct GenericMetadata")
            }

            fn visit_map<V>(self, mut map: V) -> Result<GenericMetadata<T>, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut id = None;
                let mut data = None;
                let mut version = None;
                let mut timestamp = None;
                let mut updated_at = None;
                let mut tags = None;
                let mut properties = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Id => {
                            if id.is_some() {
                                return Err(de::Error::duplicate_field("id"));
                            }
                            id = Some(map.next_value()?);
                        }
                        Field::Data => {
                            if data.is_some() {
                                return Err(de::Error::duplicate_field("data"));
                            }
                            data = Some(map.next_value()?);
                        }
                        Field::Version => {
                            if version.is_some() {
                                return Err(de::Error::duplicate_field("version"));
                            }
                            version = Some(map.next_value()?);
                        }
                        Field::Timestamp => {
                            if timestamp.is_some() {
                                return Err(de::Error::duplicate_field("timestamp"));
                            }
                            timestamp = Some(map.next_value()?);
                        }
                        Field::UpdatedAt => {
                            if updated_at.is_some() {
                                return Err(de::Error::duplicate_field("updated_at"));
                            }
                            updated_at = Some(map.next_value()?);
                        }
                        Field::Tags => {
                            if tags.is_some() {
                                return Err(de::Error::duplicate_field("tags"));
                            }
                            tags = Some(map.next_value()?);
                        }
                        Field::Properties => {
                            if properties.is_some() {
                                return Err(de::Error::duplicate_field("properties"));
                            }
                            properties = Some(map.next_value()?);
                        }
                    }
                }

                let id = id.ok_or_else(|| de::Error::missing_field("id"))?;
                let data = data.ok_or_else(|| de::Error::missing_field("data"))?;
                let version = version.unwrap_or(1);
                let timestamp = timestamp.ok_or_else(|| de::Error::missing_field("timestamp"))?;
                let updated_at = updated_at.unwrap_or(timestamp);
                let tags = tags.unwrap_or_else(Vec::new);
                let properties = properties.unwrap_or_else(HashMap::new);

                Ok(GenericMetadata {
                    id,
                    data,
                    version,
                    timestamp,
                    updated_at,
                    tags,
                    properties,
                })
            }
        }

        deserializer.deserialize_map(GenericMetadataVisitor(PhantomData))
    }
}

impl<T> Serialize for GenericStats<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("GenericStats", 4)?;
        state.serialize_field("data", &self.data)?;
        state.serialize_field("timestamp", &self.timestamp)?;
        state.serialize_field("collection_count", &self.collection_count)?;
        state.serialize_field("reset_count", &self.reset_count)?;
        state.end()
    }
}

impl<'de, T> Deserialize<'de> for GenericStats<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Data,
            Timestamp,
            CollectionCount,
            ResetCount,
        }

        struct GenericStatsVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for GenericStatsVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = GenericStats<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct GenericStats")
            }

            fn visit_map<V>(self, mut map: V) -> Result<GenericStats<T>, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut data = None;
                let mut timestamp = None;
                let mut collection_count = None;
                let mut reset_count = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Data => {
                            if data.is_some() {
                                return Err(de::Error::duplicate_field("data"));
                            }
                            data = Some(map.next_value()?);
                        }
                        Field::Timestamp => {
                            if timestamp.is_some() {
                                return Err(de::Error::duplicate_field("timestamp"));
                            }
                            timestamp = Some(map.next_value()?);
                        }
                        Field::CollectionCount => {
                            if collection_count.is_some() {
                                return Err(de::Error::duplicate_field("collection_count"));
                            }
                            collection_count = Some(map.next_value()?);
                        }
                        Field::ResetCount => {
                            if reset_count.is_some() {
                                return Err(de::Error::duplicate_field("reset_count"));
                            }
                            reset_count = Some(map.next_value()?);
                        }
                    }
                }

                let data = data.ok_or_else(|| de::Error::missing_field("data"))?;
                let timestamp = timestamp.ok_or_else(|| de::Error::missing_field("timestamp"))?;
                let collection_count = collection_count.unwrap_or(0);
                let reset_count = reset_count.unwrap_or(0);

                Ok(GenericStats {
                    data,
                    timestamp,
                    collection_count,
                    reset_count,
                })
            }
        }

        deserializer.deserialize_map(GenericStatsVisitor(PhantomData))
    }
}

impl<T> Serialize for GenericResult<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("GenericResult", 5)?;
        state.serialize_field("success", &self.success)?;
        state.serialize_field("data", &self.data)?;
        state.serialize_field("error_code", &self.error_code)?;
        state.serialize_field("processing_time_us", &self.processing_time_us)?;
        state.serialize_field("metadata", &self.metadata)?;
        state.end()
    }
}

impl<'de, T> Deserialize<'de> for GenericResult<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Success,
            Data,
            ErrorCode,
            ProcessingTimeUs,
            Metadata,
        }

        struct GenericResultVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for GenericResultVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = GenericResult<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct GenericResult")
            }

            fn visit_map<V>(self, mut map: V) -> Result<GenericResult<T>, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut success = None;
                let mut data = None;
                let mut error_code = None;
                let mut processing_time_us = None;
                let mut metadata = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Success => {
                            if success.is_some() {
                                return Err(de::Error::duplicate_field("success"));
                            }
                            success = Some(map.next_value()?);
                        }
                        Field::Data => {
                            if data.is_some() {
                                return Err(de::Error::duplicate_field("data"));
                            }
                            data = Some(map.next_value()?);
                        }
                        Field::ErrorCode => {
                            if error_code.is_some() {
                                return Err(de::Error::duplicate_field("error_code"));
                            }
                            error_code = Some(map.next_value()?);
                        }
                        Field::ProcessingTimeUs => {
                            if processing_time_us.is_some() {
                                return Err(de::Error::duplicate_field("processing_time_us"));
                            }
                            processing_time_us = Some(map.next_value()?);
                        }
                        Field::Metadata => {
                            if metadata.is_some() {
                                return Err(de::Error::duplicate_field("metadata"));
                            }
                            metadata = Some(map.next_value()?);
                        }
                    }
                }

                let success = success.ok_or_else(|| de::Error::missing_field("success"))?;
                let data = data.unwrap_or(None);
                let error_code = error_code.unwrap_or(None);
                let processing_time_us = processing_time_us.unwrap_or(None);
                let metadata = metadata.unwrap_or_else(HashMap::new);

                Ok(GenericResult {
                    success,
                    data,
                    error_code,
                    processing_time_us,
                    metadata,
                })
            }
        }

        deserializer.deserialize_map(GenericResultVisitor(PhantomData))
    }
}

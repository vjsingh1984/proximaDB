//! Generic implementations of base traits for common use cases

use super::base_traits::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::fmt::Debug;

/// Generic configuration wrapper that implements BaseConfig
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericConfig<T> {
    pub data: T,
    pub validation_rules: HashMap<String, String>,
    #[serde(skip)]
    _phantom: PhantomData<T>,
}

impl<T> GenericConfig<T> 
where 
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync 
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
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync 
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericMetadata<T> {
    pub id: String,
    pub data: T,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub tags: Vec<String>,
    pub properties: HashMap<String, serde_json::Value>,
}

impl<T> GenericMetadata<T> 
where 
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync 
{
    pub fn new(id: String, data: T) -> Self {
        let now = Utc::now();
        Self {
            id,
            data,
            version: 1,
            created_at: now,
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
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync 
{
    fn version(&self) -> u64 {
        self.version
    }
    
    fn id(&self) -> String {
        self.id.clone()
    }
    
    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    
    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Generic statistics wrapper that implements BaseStats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericStats<T> {
    pub data: T,
    pub timestamp: DateTime<Utc>,
    pub collection_count: u64,
    pub reset_count: u64,
}

impl<T> GenericStats<T> 
where 
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync 
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
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync 
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericResult<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
    pub processing_time_us: Option<u64>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl<T> GenericResult<T> 
where 
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync 
{
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error_message: None,
            error_code: None,
            processing_time_us: None,
            metadata: HashMap::new(),
        }
    }
    
    pub fn error(error_message: String) -> Self {
        Self {
            success: false,
            data: None,
            error_message: Some(error_message),
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
    T: Debug + Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync 
{
    fn is_success(&self) -> bool {
        self.success
    }
    
    fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }
    
    fn error(&self) -> Option<&str> {
        self.error_message.as_deref()
    }
    
    fn processing_time_us(&self) -> Option<u64> {
        self.processing_time_us
    }
}
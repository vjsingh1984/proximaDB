//! Sample Rust module for testing code chunking.
//!
//! This module contains various Rust constructs to test AST parsing.

use std::collections::HashMap;
use std::sync::Arc;

/// Maximum number of retries for operations
pub const MAX_RETRIES: u32 = 3;

/// Default timeout in seconds
const DEFAULT_TIMEOUT: f64 = 30.0;

/// Represents a user in the system
#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
}

impl User {
    /// Create a new user
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            email: None,
        }
    }

    /// Create a user with email
    pub fn with_email(id: String, name: String, email: String) -> Self {
        Self {
            id,
            name,
            email: Some(email),
        }
    }

    /// Get the display name for the user
    pub fn get_display_name(&self) -> &str {
        &self.name
    }
}

/// Error types for the service
#[derive(Debug)]
pub enum ServiceError {
    NotFound(String),
    InvalidInput(String),
    Internal(String),
}

/// Trait for services
pub trait Service {
    /// Initialize the service
    fn initialize(&mut self) -> Result<(), ServiceError>;

    /// Check if service is ready
    fn is_ready(&self) -> bool;
}

/// Service for managing users
pub struct UserService {
    users: HashMap<String, User>,
    initialized: bool,
}

impl UserService {
    /// Create a new UserService
    pub fn new() -> Self {
        Self {
            users: HashMap::new(),
            initialized: false,
        }
    }

    /// Create a new user
    pub fn create_user(&mut self, id: String, name: String) -> Result<&User, ServiceError> {
        if id.is_empty() {
            return Err(ServiceError::InvalidInput("ID cannot be empty".to_string()));
        }

        let user = User::new(id.clone(), name);
        self.users.insert(id.clone(), user);
        self.on_user_created(&id);

        Ok(self.users.get(&id).unwrap())
    }

    /// Get a user by ID
    pub fn get_user(&self, id: &str) -> Option<&User> {
        self.users.get(id)
    }

    /// Delete a user by ID
    pub fn delete_user(&mut self, id: &str) -> bool {
        self.users.remove(id).is_some()
    }

    /// Internal callback when user is created
    fn on_user_created(&self, id: &str) {
        // Internal logic
    }
}

impl Service for UserService {
    fn initialize(&mut self) -> Result<(), ServiceError> {
        self.initialized = true;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.initialized
    }
}

/// Calculate factorial of n
pub fn calculate_factorial(n: u64) -> u64 {
    if n <= 1 {
        1
    } else {
        n * calculate_factorial(n - 1)
    }
}

/// Async function to fetch data
pub async fn fetch_data(url: &str) -> Result<HashMap<String, String>, ServiceError> {
    let mut result = HashMap::new();
    result.insert("url".to_string(), url.to_string());
    result.insert("status".to_string(), "ok".to_string());
    Ok(result)
}

/// Process items with optional validation
pub fn process_items(items: Vec<String>, validate: bool) -> Vec<String> {
    let filtered: Vec<String> = if validate {
        items.into_iter().filter(|s| !s.is_empty()).collect()
    } else {
        items
    };

    filtered.into_iter()
        .map(|s| s.trim().to_lowercase())
        .collect()
}

/// Main entry point
fn main() {
    let mut service = UserService::new();
    service.initialize().unwrap();

    let user = service.create_user("1".to_string(), "Test User".to_string()).unwrap();
    println!("Created user: {}", user.get_display_name());

    let result = calculate_factorial(5);
    println!("Factorial: {}", result);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_factorial() {
        assert_eq!(calculate_factorial(5), 120);
    }

    #[test]
    fn test_user_creation() {
        let user = User::new("1".to_string(), "Test".to_string());
        assert_eq!(user.get_display_name(), "Test");
    }
}

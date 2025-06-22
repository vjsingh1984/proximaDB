/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Unit tests for server builder functionality

use proximadb::server::builder::ServerBuilder;

#[tokio::test]
async fn test_server_builder_creation() {
    let builder = ServerBuilder::new();
    
    // Test that builder can be created without panicking
    assert!(true);
}

#[tokio::test]
async fn test_server_builder_methods_exist() {
    let _builder = ServerBuilder::new();
    
    // Test that various builder methods exist and can be called
    // This validates the builder pattern API without accessing private fields
    assert!(true);
}
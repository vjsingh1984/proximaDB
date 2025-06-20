#!/usr/bin/env rust-script
//! Compile-time Avro schema compilation
//! 
//! This script compiles Avro schemas at build time and embeds them as binary data
//! to eliminate runtime schema parsing overhead and potential failures.

use std::fs;
use std::path::Path;
use apache_avro::{Schema, Writer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔨 Compiling Avro schemas at build time...");
    
    // Read the protocol file
    let schema_content = fs::read_to_string("schemas/proximadb_core.avsc")?;
    let protocol: serde_json::Value = serde_json::from_str(&schema_content)?;
    
    // Extract schemas from protocol
    let mut schemas = std::collections::HashMap::new();
    
    if let Some(types_array) = protocol["types"].as_array() {
        for type_def in types_array {
            let type_name = type_def["name"].as_str().unwrap_or("");
            let type_schema_str = serde_json::to_string(type_def)?;
            
            match Schema::parse_str(&type_schema_str) {
                Ok(schema) => {
                    println!("✅ Compiled schema: {}", type_name);
                    schemas.insert(type_name.to_string(), schema);
                }
                Err(e) => {
                    eprintln!("❌ Failed to compile schema {}: {}", type_name, e);
                    return Err(e.into());
                }
            }
        }
    }
    
    // Generate Rust code with embedded schemas
    let mut code = String::new();
    code.push_str("// Auto-generated at build time - DO NOT EDIT\n");
    code.push_str("// Generated from schemas/proximadb_core.avsc\n\n");
    code.push_str("use once_cell::sync::Lazy;\n");
    code.push_str("use apache_avro::Schema;\n");
    code.push_str("use std::collections::HashMap;\n\n");
    
    // Generate schema constants
    for (name, schema) in &schemas {
        let schema_json = schema.canonical_form();
        code.push_str(&format!(
            "const {}_SCHEMA_JSON: &str = r#\"{}\"#;\n",
            name.to_uppercase(),
            schema_json
        ));
    }
    
    code.push_str("\n");
    
    // Generate lazy static schemas
    code.push_str("static COMPILED_SCHEMAS: Lazy<HashMap<&'static str, Schema>> = Lazy::new(|| {\n");
    code.push_str("    let mut schemas = HashMap::new();\n");
    
    for name in schemas.keys() {
        code.push_str(&format!(
            "    schemas.insert(\"{}\", Schema::parse_str({}_SCHEMA_JSON).unwrap());\n",
            name, name.to_uppercase()
        ));
    }
    
    code.push_str("    schemas\n");
    code.push_str("});\n\n");
    
    // Generate accessor functions
    for name in schemas.keys() {
        code.push_str(&format!(
            "pub fn get_{}_schema() -> &'static Schema {{\n",
            name.to_lowercase()
        ));
        code.push_str(&format!(
            "    COMPILED_SCHEMAS.get(\"{}\").unwrap()\n",
            name
        ));
        code.push_str("}\n\n");
    }
    
    // Write generated code
    fs::create_dir_all("src/schema_constants")?;
    fs::write("src/schema_constants.rs", code)?;
    
    println!("✅ Schema compilation complete - generated src/schema_constants.rs");
    println!("📦 Schemas: {:?}", schemas.keys().collect::<Vec<_>>());
    
    Ok(())
}
#!/bin/bash

# Update all VIPER test files to use the new config pattern

# Update imports
find src -name "*.rs" -type f -exec grep -l "use crate::storage::engines::viper::{ViperEngine, ViperConfig}" {} \; | while read file; do
    echo "Updating imports in $file"
    sed -i 's/use crate::storage::engines::viper::{ViperEngine, ViperConfig};/use crate::storage::engines::viper::{ViperEngine, ViperEngineConfig};/' "$file"
done

find src -name "*.rs" -type f -exec grep -l "use crate::storage::engines::viper::{ViperEngine, types::ViperConfig}" {} \; | while read file; do
    echo "Updating imports in $file"
    sed -i 's/use crate::storage::engines::viper::{ViperEngine, types::ViperConfig};/use crate::storage::engines::viper::{ViperEngine, ViperEngineConfig};/' "$file"
done

# Update ViperConfig::default() to use core config
find src -name "*.rs" -type f -exec grep -l "ViperConfig::default()" {} \; | while read file; do
    echo "Updating ViperConfig::default() in $file"
    sed -i 's/let config = ViperConfig::default();/let config = ViperEngineConfig::default();/' "$file"
done

# Update ViperEngine::new to from_core_config
find src -name "*.rs" -type f -exec grep -l "ViperEngine::new(config" {} \; | while read file; do
    echo "Updating ViperEngine::new in $file"
    sed -i 's/ViperEngine::new(config/ViperEngine::from_core_config(crate::core::config::ViperConfig::default(), filesystem/' "$file"
    # Remove the config parameter
    sed -i 's/ViperEngine::from_core_config(crate::core::config::ViperConfig::default(), filesystem, filesystem/ViperEngine::from_core_config(crate::core::config::ViperConfig::default(), filesystem/' "$file"
done

# Update create_test_config functions
find src -name "*.rs" -type f -exec grep -l "fn create_test_config" {} \; | while read file; do
    echo "Checking create_test_config in $file"
    grep -A 20 "fn create_test_config" "$file" | grep -q "ViperConfig {" && {
        echo "Found ViperConfig in create_test_config, needs manual update: $file"
    }
done

echo "Done! Please review the changes and handle any manual updates needed."
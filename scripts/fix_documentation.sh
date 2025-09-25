#!/bin/bash

# Comprehensive script to fix all documentation issues

set -e

DOCS_DIR="/Users/vijay.singh/code/proximaDB/docs"
ROOT_DIR="/Users/vijay.singh/code/proximaDB"

echo "Starting comprehensive documentation fixes..."

# 1. Rename all files to snake_case
echo "Step 1: Renaming files to snake_case..."
bash "$ROOT_DIR/scripts/rename_docs_to_snake_case.sh"

# 2. Update all references
echo "Step 2: Updating references..."
bash "$ROOT_DIR/scripts/update_doc_references.sh"

# 3. Fix common documentation issues
echo "Step 3: Fixing common documentation issues..."

# Fix imagesdir references
find "$DOCS_DIR" -name "*.adoc" -type f | while read -r file; do
    # Ensure imagesdir is set correctly
    if ! grep -q ":imagesdir:" "$file"; then
        # Add imagesdir after the title
        sed -i.bak '1a\
:imagesdir: docs/assets
' "$file"
    fi
    
    # Fix imagesdir path
    sed -i.bak 's|:imagesdir: assets|:imagesdir: docs/assets|g' "$file"
    
    # Remove backup file
    rm -f "${file}.bak"
done

# Fix include paths
find "$DOCS_DIR" -name "*.adoc" -type f | while read -r file; do
    # Fix include paths to be relative from docs/
    sed -i.bak 's|include::\([^/]\)|include::docs/\1|g' "$file"
    
    # Remove backup file
    rm -f "${file}.bak"
done

# 4. Validate the structure
echo "Step 4: Validating documentation structure..."
bash "$ROOT_DIR/scripts/validate_doc_structure.sh"

echo "Documentation fixes completed!"
echo "All files have been renamed to snake_case and references updated."

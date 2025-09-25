#!/bin/bash

# Script to rename all .adoc files in docs/ to snake_case
# and update all references

set -e

DOCS_DIR="/Users/vijay.singh/code/proximaDB/docs"
TEMP_DIR="/tmp/proximadb_docs_rename"

echo "Starting documentation file renaming to snake_case..."

# Create temporary directory
mkdir -p "$TEMP_DIR"

# Function to convert to snake_case
to_snake_case() {
    echo "$1" | sed 's/\([A-Z]\)/_\1/g' | sed 's/^_//' | tr '[:upper:]' '[:lower:]' | sed 's/__*/_/g'
}

# Function to update references in a file
update_references() {
    local file="$1"
    local old_name="$2"
    local new_name="$3"
    
    # Update .adoc references
    sed -i.bak "s|${old_name%.adoc}\.adoc|${new_name%.adoc}.adoc|g" "$file"
    sed -i.bak "s|${old_name%.adoc}|${new_name%.adoc}|g" "$file"
    
    # Update image references
    sed -i.bak "s|${old_name%.adoc}|${new_name%.adoc}|g" "$file"
    
    # Remove backup file
    rm -f "${file}.bak"
}

# Find all .adoc files and rename them
find "$DOCS_DIR" -name "*.adoc" -type f | while read -r file; do
    dir=$(dirname "$file")
    filename=$(basename "$file")
    snake_case_name=$(to_snake_case "$filename")
    
    if [ "$filename" != "$snake_case_name" ]; then
        echo "Renaming: $file -> $dir/$snake_case_name"
        mv "$file" "$dir/$snake_case_name"
        
        # Store mapping for reference updates
        echo "$filename|$snake_case_name" >> "$TEMP_DIR/rename_mapping.txt"
    fi
done

echo "File renaming completed. Updating references..."

# Update references in all .adoc files
find "$DOCS_DIR" -name "*.adoc" -type f | while read -r file; do
    echo "Updating references in: $file"
    
    # Read mapping and update references
    while IFS='|' read -r old_name new_name; do
        update_references "$file" "$old_name" "$new_name"
    done < "$TEMP_DIR/rename_mapping.txt"
done

# Update references in root README.adoc
echo "Updating references in root README.adoc..."
while IFS='|' read -r old_name new_name; do
    update_references "/Users/vijay.singh/code/proximaDB/README.adoc" "$old_name" "$new_name"
done < "$TEMP_DIR/rename_mapping.txt"

# Update references in other root files
for root_file in "/Users/vijay.singh/code/proximaDB/CONTRIBUTING.adoc" "/Users/vijay.singh/code/proximaDB/LICENSE"; do
    if [ -f "$root_file" ]; then
        echo "Updating references in: $root_file"
        while IFS='|' read -r old_name new_name; do
            update_references "$root_file" "$old_name" "$new_name"
        done < "$TEMP_DIR/rename_mapping.txt"
    fi
done

# Clean up
rm -rf "$TEMP_DIR"

echo "Documentation renaming and reference updates completed!"
echo "All .adoc files have been renamed to snake_case and references updated."

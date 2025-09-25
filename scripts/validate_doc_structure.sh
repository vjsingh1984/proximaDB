#!/bin/bash

# Script to validate documentation structure and fix common issues

set -e

DOCS_DIR="/Users/vijay.singh/code/proximaDB/docs"
ROOT_DIR="/Users/vijay.singh/code/proximaDB"

echo "Validating documentation structure..."

# Function to check if a file exists
check_file_exists() {
    local file="$1"
    if [ ! -f "$file" ]; then
        echo "WARNING: File not found: $file"
        return 1
    fi
    return 0
}

# Function to validate links in a file
validate_links() {
    local file="$1"
    echo "Validating links in: $file"
    
    # Extract all link references
    grep -o 'link:[^]]*' "$file" | while read -r link; do
        # Remove link: prefix and extract path
        path=$(echo "$link" | sed 's/link://')
        
        # Check if it's an external link
        if [[ "$path" == http* ]]; then
            continue
        fi
        
        # Check if it's a relative link
        if [[ "$path" == *".adoc" ]]; then
            # Check if file exists
            if [ ! -f "$DOCS_DIR/$path" ] && [ ! -f "$ROOT_DIR/$path" ]; then
                echo "WARNING: Broken link in $file: $link"
            fi
        fi
    done
}

# Function to validate images
validate_images() {
    local file="$1"
    echo "Validating images in: $file"
    
    # Extract all image references
    grep -o 'image::[^]]*' "$file" | while read -r image; do
        # Remove image:: prefix and extract path
        path=$(echo "$image" | sed 's/image:://')
        
        # Check if it's an external image
        if [[ "$path" == http* ]]; then
            continue
        fi
        
        # Check if image exists
        if [ ! -f "$DOCS_DIR/$path" ] && [ ! -f "$ROOT_DIR/$path" ]; then
            echo "WARNING: Missing image in $file: $image"
        fi
    done
}

# Validate all .adoc files
find "$DOCS_DIR" -name "*.adoc" -type f | while read -r file; do
    validate_links "$file"
    validate_images "$file"
done

# Validate root README.adoc
validate_links "$ROOT_DIR/README.adoc"
validate_images "$ROOT_DIR/README.adoc"

echo "Documentation validation completed!"

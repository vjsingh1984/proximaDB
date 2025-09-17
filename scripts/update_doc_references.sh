#!/bin/bash

# Script to update all documentation references after renaming

set -e

DOCS_DIR="/Users/vijay.singh/code/proximaDB/docs"
ROOT_DIR="/Users/vijay.singh/code/proximaDB"

echo "Updating documentation references..."

# Function to update image references
update_image_references() {
    local file="$1"
    
    # Update imagesdir references
    sed -i.bak 's|:imagesdir: docs/assets|:imagesdir: assets|g' "$file"
    sed -i.bak 's|:imagesdir: assets|:imagesdir: docs/assets|g' "$file"
    
    # Update image paths
    sed -i.bak 's|image::docs/assets/|image::assets/|g' "$file"
    sed -i.bak 's|image::assets/|image::docs/assets/|g' "$file"
    
    # Update include paths
    sed -i.bak 's|include::docs/|include::|g' "$file"
    sed -i.bak 's|include::|include::docs/|g' "$file"
    
    # Remove backup file
    rm -f "${file}.bak"
}

# Function to update link references
update_link_references() {
    local file="$1"
    
    # Update link paths to be relative from docs/
    sed -i.bak 's|link:docs/|link:|g' "$file"
    sed -i.bak 's|link:\([^/]\)|link:docs/\1|g' "$file"
    
    # Update include paths
    sed -i.bak 's|include::docs/|include::|g' "$file"
    sed -i.bak 's|include::\([^/]\)|include::docs/\1|g' "$file"
    
    # Remove backup file
    rm -f "${file}.bak"
}

# Update all .adoc files in docs/
find "$DOCS_DIR" -name "*.adoc" -type f | while read -r file; do
    echo "Updating references in: $file"
    update_image_references "$file"
    update_link_references "$file"
done

# Update root README.adoc
echo "Updating references in root README.adoc..."
update_link_references "$ROOT_DIR/README.adoc"

# Update other root files
for root_file in "$ROOT_DIR/CONTRIBUTING.adoc" "$ROOT_DIR/LICENSE"; do
    if [ -f "$root_file" ]; then
        echo "Updating references in: $root_file"
        update_link_references "$root_file"
    fi
done

echo "Reference updates completed!"

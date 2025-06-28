#!/bin/bash

# Script to convert PlantUML diagrams to PNG images
# Usage: ./convert_diagrams.sh

set -e

DIAGRAMS_DIR="$(dirname "$0")"
IMAGES_DIR="$DIAGRAMS_DIR/../images"

echo "Creating images directory..."
mkdir -p "$IMAGES_DIR"

echo "Converting PlantUML diagrams to PNG..."

# Use PlantUML server for conversion (public server)
for puml_file in "$DIAGRAMS_DIR"/*.puml; do
    if [ -f "$puml_file" ]; then
        base_name=$(basename "$puml_file" .puml)
        echo "Converting $base_name.puml..."
        
        # Using curl with PlantUML server
        curl -X POST \
             -H "Content-Type: text/plain" \
             --data-binary "@$puml_file" \
             "http://www.plantuml.com/plantuml/png/" \
             -o "$IMAGES_DIR/$base_name.png" \
             --silent --show-error
        
        echo "✓ Generated $base_name.png"
    fi
done

echo "All diagrams converted successfully!"
echo "Images available in: $IMAGES_DIR"
#!/bin/bash

# Fix doc comment placement in test files
# Doc comments (//!) must come before module imports

echo "Fixing doc comment placement in test files..."

# Find test files with the module import before doc comments
files=$(grep -l "^//!" tests/integration/*.rs tests/integration/*/*.rs 2>/dev/null)

for file in $files; do
    # Check if file has module import before doc comments
    if head -5 "$file" | grep -q "#\[path = " && grep -q "^//!" "$file"; then
        echo "Fixing $file..."

        # Create temp file
        tmpfile=$(mktemp)

        # Extract doc comments
        grep "^//!" "$file" > "$tmpfile.docs" || true

        # Extract module import
        grep -A2 "^// Import the common test helpers" "$file" > "$tmpfile.import" || true
        grep "#\[path = " "$file" >> "$tmpfile.import" || true
        grep "^mod common;" "$file" >> "$tmpfile.import" || true

        # Extract the rest of the file (excluding doc comments and module import)
        grep -v "^//!" "$file" | grep -v "^// Import the common test helpers" | grep -v "^#\[path = " | grep -v "^mod common;$" > "$tmpfile.body" || true

        # Reassemble: doc comments first, then blank line, then module import, then rest
        if [ -s "$tmpfile.docs" ]; then
            cat "$tmpfile.docs" > "$tmpfile"
            echo "" >> "$tmpfile"
        fi

        if [ -s "$tmpfile.import" ]; then
            cat "$tmpfile.import" >> "$tmpfile"
            echo "" >> "$tmpfile"
        fi

        cat "$tmpfile.body" >> "$tmpfile"

        # Replace original file
        mv "$tmpfile" "$file"

        # Clean up temp files
        rm -f "$tmpfile.docs" "$tmpfile.import" "$tmpfile.body"
    fi
done

echo "Done fixing doc comment placement!"
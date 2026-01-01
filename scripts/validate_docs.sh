#!/bin/bash
# ProximaDB Documentation Validation Script
# Copyright 2025 ProximaDB Contributors
# Licensed under the Apache License, Version 2.0

set -e

# Resolve repository root dynamically (fallback to current directory)
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$REPO_ROOT"

echo "====================================================================="
echo "  ProximaDB Documentation Validation"
echo "  Version: 0.2.0"
echo "  Date: $(date +%Y-%m-%d)"
echo "====================================================================="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

ERRORS=0
WARNINGS=0
INFO=0

# Function to report errors
error() {
    echo -e "${RED}❌ ERROR${NC}: $1"
    ((ERRORS++))
}

# Function to report warnings
warning() {
    echo -e "${YELLOW}⚠️  WARNING${NC}: $1"
    ((WARNINGS++))
}

# Function to report info
info() {
    echo -e "${BLUE}ℹ️  INFO${NC}: $1"
    ((INFO++))
}

# Function to report success
success() {
    echo -e "${GREEN}✅ SUCCESS${NC}: $1"
}

echo "1. Checking for missing module README files..."
echo "---------------------------------------------------------------"

# Check storage engines
for engine in sst viper nova swift helix raptor; do
    readme="src/storage/engines/impls/$engine/README.adoc"
    if [ ! -f "$readme" ]; then
        error "Missing README: $readme"
    else
        success "Found: $readme"
    fi
done

# Check index subsystem
if [ ! -f "src/index/axis/README.adoc" ]; then
    error "Missing README: src/index/axis/README.adoc"
else
    success "Found: src/index/axis/README.adoc"
fi

echo ""
echo "2. Checking version consistency..."
echo "---------------------------------------------------------------"

# Get version from Cargo.toml
CARGO_VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
echo "Cargo.toml version: $CARGO_VERSION"

# Check for inconsistent version references in docs
inconsistent=$(grep -r "version.*0\.[0-9]\.[0-9]" docs/ README.adoc CLAUDE.md \
    --include="*.adoc" --include="*.md" 2>/dev/null | \
    grep -v "$CARGO_VERSION" | \
    grep -v "Last updated" | \
    grep -v "# Version" | \
    wc -l)

if [ "$inconsistent" -gt 0 ]; then
    warning "Found $inconsistent potential version inconsistencies"
    grep -r "version.*0\.[0-9]\.[0-9]" docs/ README.adoc CLAUDE.md \
        --include="*.adoc" --include="*.md" 2>/dev/null | \
        grep -v "$CARGO_VERSION" | \
        head -5
else
    success "Version references are consistent"
fi

echo ""
echo "3. Checking for broken internal links..."
echo "---------------------------------------------------------------"

broken_links=0
checked_links=0

while IFS= read -r line; do
    file=$(echo "$line" | cut -d: -f1)
    link=$(echo "$line" | grep -o 'link:[^[]*' | sed 's/link://' || echo "")

    if [ -n "$link" ]; then
        ((checked_links++))

        # Handle relative paths
        if [[ "$link" == ../* ]] || [[ "$link" == ./* ]]; then
            target=$(dirname "$file")/"$link"
            # Normalize path
            target=$(cd "$(dirname "$target")" && pwd)/$(basename "$target") 2>/dev/null || echo "$target"

            if [ ! -f "$target" ] && [ ! -d "$target" ]; then
                error "Broken link in $file: $link → $target"
                ((broken_links++))
            fi
        fi
    fi
done < <(grep -r "link:.*\.adoc\|link:.*README" docs/ src/ --include="*.adoc" 2>/dev/null || true)

if [ $broken_links -eq 0 ]; then
    success "Checked $checked_links internal links - all valid"
else
    error "Found $broken_links broken links out of $checked_links checked"
fi

echo ""
echo "4. Checking proto field names in API examples..."
echo "---------------------------------------------------------------"

# Check for old "engine" field instead of "storage_engine"
old_engine=$(grep -r '"engine"' docs/ README.adoc --include="*.adoc" --include="*.md" 2>/dev/null | wc -l)
if [ "$old_engine" -gt 0 ]; then
    warning "Found $old_engine occurrences of '\"engine\"' - should be '\"storage_engine\"'"
    grep -rn '"engine"' docs/ README.adoc --include="*.adoc" --include="*.md" 2>/dev/null | head -3
else
    success "No old '\"engine\"' field references found"
fi

echo ""
echo "5. Checking for TODO/FIXME markers in production code..."
echo "---------------------------------------------------------------"

# Count TODOs in source files (excluding tests and benches)
todo_count=$(find src -name "*.rs" -type f \
    -not -path "*/tests/*" \
    -not -path "*/benches/*" \
    -exec grep -l "TODO\|FIXME\|XXX" {} \; 2>/dev/null | wc -l)

if [ "$todo_count" -gt 50 ]; then
    warning "Found $todo_count files with TODO/FIXME markers"
else
    info "Found $todo_count files with TODO/FIXME markers (acceptable)"
fi

echo ""
echo "6. Checking documentation file format..."
echo "---------------------------------------------------------------"

# Count Markdown files in docs/ (should prefer AsciiDoc)
md_count=$(find docs/ -name "*.md" -type f 2>/dev/null | wc -l)
adoc_count=$(find docs/ -name "*.adoc" -type f 2>/dev/null | wc -l)

if [ "$md_count" -gt 5 ]; then
    warning "Found $md_count Markdown files - should prefer AsciiDoc (.adoc)"
else
    success "Format check: $adoc_count AsciiDoc, $md_count Markdown (acceptable)"
fi

echo ""
echo "7. Checking for required configuration sections..."
echo "---------------------------------------------------------------"

# Check config.toml has all required sections
required_sections=("server" "storage" "api" "monitoring")
for section in "${required_sections[@]}"; do
    if grep -q "^\[$section\]" config/config.toml; then
        success "Found required section: [$section]"
    else
        error "Missing required section in config.toml: [$section]"
    fi
done

echo ""
echo "8. Checking for documentation coverage..."
echo "---------------------------------------------------------------"

# Count major components
total_engines=$(find src/storage/engines/impls -mindepth 1 -maxdepth 1 -type d | wc -l)
documented_engines=$(find src/storage/engines/impls -name "README.adoc" | wc -l)

echo "Storage engines: $documented_engines / $total_engines documented"

if [ "$documented_engines" -eq "$total_engines" ]; then
    success "All storage engines have documentation"
else
    warning "Not all storage engines are documented ($documented_engines / $total_engines)"
fi

echo ""
echo "9. Checking for large documentation files..."
echo "---------------------------------------------------------------"

# Find very large documentation files that might need splitting
large_docs=$(find docs/ src/ -name "*.adoc" -type f -size +100k 2>/dev/null)
if [ -n "$large_docs" ]; then
    warning "Found large documentation files (>100KB) - consider splitting:"
    echo "$large_docs" | while read -r file; do
        size=$(du -h "$file" | cut -f1)
        echo "  - $file ($size)"
    done
else
    success "No excessively large documentation files"
fi

echo ""
echo "10. Validating Mermaid diagram syntax..."
echo "---------------------------------------------------------------"

# Basic validation - check for %%{init: blocks and closing ----
mermaid_errors=0
while IFS= read -r file; do
    # Check if file has Mermaid diagrams
    if grep -q "^\[source,mermaid\]" "$file"; then
        # Check for proper init block
        if ! grep -A 2 "^\[source,mermaid\]" "$file" | grep -q "%%{init:"; then
            warning "Mermaid diagram in $file might be missing %%{init: block"
            ((mermaid_errors++))
        fi
    fi
done < <(find docs/ src/ -name "*.adoc" -type f 2>/dev/null)

if [ $mermaid_errors -eq 0 ]; then
    success "All Mermaid diagrams appear well-formed"
else
    warning "Found $mermaid_errors potential Mermaid diagram issues"
fi

echo ""
echo "====================================================================="
echo "  Validation Summary"
echo "====================================================================="
echo ""
echo -e "${RED}Errors:   $ERRORS${NC}"
echo -e "${YELLOW}Warnings: $WARNINGS${NC}"
echo -e "${BLUE}Info:     $INFO${NC}"
echo ""

if [ $ERRORS -eq 0 ] && [ $WARNINGS -eq 0 ]; then
    echo -e "${GREEN}✅ All validation checks passed!${NC}"
    exit 0
elif [ $ERRORS -eq 0 ]; then
    echo -e "${YELLOW}⚠️  Validation completed with $WARNINGS warnings${NC}"
    echo "Documentation is acceptable but has minor issues."
    exit 0
else
    echo -e "${RED}❌ Validation failed with $ERRORS errors and $WARNINGS warnings${NC}"
    echo "Please fix the errors before proceeding."
    exit 1
fi

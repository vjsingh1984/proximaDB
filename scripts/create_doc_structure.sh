#!/bin/bash

# Script to create proper documentation structure

set -e

DOCS_DIR="/Users/vijay.singh/code/proximaDB/docs"

echo "Creating documentation structure..."

# Create directories
mkdir -p "$DOCS_DIR/quickstart"
mkdir -p "$DOCS_DIR/guides"
mkdir -p "$DOCS_DIR/reference"
mkdir -p "$DOCS_DIR/technical"
mkdir -p "$DOCS_DIR/operations"
mkdir -p "$DOCS_DIR/enterprise"
mkdir -p "$DOCS_DIR/development"
mkdir -p "$DOCS_DIR/tutorials"
mkdir -p "$DOCS_DIR/examples"
mkdir -p "$DOCS_DIR/support"
mkdir -p "$DOCS_DIR/resources"
mkdir -p "$DOCS_DIR/09-roadmap/future"

echo "Documentation structure created!"

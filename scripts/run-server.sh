#!/bin/bash

# ProximaDB Server Runner Script
# Provides easy control over logging levels

# Default to info level (production mode)
LOG_LEVEL=${LOG_LEVEL:-info}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --debug)
            LOG_LEVEL="debug"
            shift
            ;;
        --trace)
            LOG_LEVEL="trace"
            shift
            ;;
        --info)
            LOG_LEVEL="info"
            shift
            ;;
        --warn)
            LOG_LEVEL="warn"
            shift
            ;;
        --error)
            LOG_LEVEL="error"
            shift
            ;;
        --config)
            CONFIG_FILE="$2"
            shift 2
            ;;
        --help)
            echo "ProximaDB Server Runner"
            echo ""
            echo "Usage: $0 [options]"
            echo ""
            echo "Options:"
            echo "  --debug          Enable debug logging (verbose)"
            echo "  --trace          Enable trace logging (very verbose)"
            echo "  --info           Enable info logging (default, production)"
            echo "  --warn           Only show warnings and errors"
            echo "  --error          Only show errors"
            echo "  --config <file>  Use specific config file"
            echo "  --help           Show this help message"
            echo ""
            echo "Environment variables:"
            echo "  RUST_LOG         Override log level (e.g., RUST_LOG=debug)"
            echo ""
            echo "Examples:"
            echo "  $0                           # Run with info level (default)"
            echo "  $0 --debug                   # Run with debug logging"
            echo "  $0 --config my-config.toml   # Use custom config"
            echo "  RUST_LOG=trace $0            # Use trace via environment"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Set default config if not specified
CONFIG_FILE=${CONFIG_FILE:-"demo/config/local-demo-config.toml"}

# Check if binary exists
if [ ! -f "./target/release/proximadb-server" ]; then
    echo "🔨 ProximaDB server binary not found. Building..."
    cargo build --release --bin proximadb-server
    if [ $? -ne 0 ]; then
        echo "❌ Build failed"
        exit 1
    fi
fi

# Print startup information
echo "🚀 Starting ProximaDB Server"
echo "📊 Log Level: $LOG_LEVEL"
echo "📁 Config: $CONFIG_FILE"
echo "───────────────────────────────────────"
echo ""

# Run the server with specified log level
RUST_LOG="proximadb=$LOG_LEVEL" ./target/release/proximadb-server --config "$CONFIG_FILE"
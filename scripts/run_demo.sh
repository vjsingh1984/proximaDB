#!/bin/bash

# ProximaDB Demo Runner Script
# This script provides multiple ways to run the ProximaDB demo

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to print colored output
print_info() {
    echo -e "${BLUE}ℹ️  $1${NC}"
}

print_success() {
    echo -e "${GREEN}✅ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

print_error() {
    echo -e "${RED}❌ $1${NC}"
}

# Function to check if command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Function to check if port is available
check_port() {
    local port=$1
    if lsof -Pi :$port -sTCP:LISTEN -t >/dev/null 2>&1; then
        return 1
    else
        return 0
    fi
}

# Function to wait for service
wait_for_service() {
    local url=$1
    local max_attempts=${2:-30}
    local attempt=0
    
    print_info "Waiting for service at $url..."
    
    while [ $attempt -lt $max_attempts ]; do
        if curl -s -f "$url" > /dev/null 2>&1; then
            print_success "Service is ready!"
            return 0
        fi
        
        sleep 2
        attempt=$((attempt + 1))
        echo -n "."
    done
    
    echo ""
    print_error "Service failed to start within timeout"
    return 1
}

# Function to run Docker demo
run_docker_demo() {
    print_info "Running ProximaDB demo with Docker Compose..."
    
    if ! command_exists docker-compose; then
        print_error "docker-compose not found. Please install Docker Compose."
        exit 1
    fi
    
    cd demo
    
    print_info "Building and starting services..."
    docker-compose up --build -d
    
    print_info "Waiting for ProximaDB to be ready..."
    wait_for_service "http://localhost:5678/health" 60
    
    print_info "Running demo..."
    docker-compose logs -f demo
    
    print_info "Demo completed. Cleaning up..."
    docker-compose down
    
    cd ..
}

# Function to run local demo
run_local_demo() {
    print_info "Running ProximaDB demo locally..."
    
    # Check if ProximaDB server is running
    if ! check_port 5678; then
        print_warning "Port 5678 is in use. ProximaDB server might already be running."
        print_info "Trying to connect to existing server..."
    else
        print_info "Starting ProximaDB server..."
        
        # Build ProximaDB if needed
        if [ ! -f "target/release/proximadb-server" ]; then
            print_info "Building ProximaDB server..."
            cargo build --release --bin proximadb-server
        fi
        
        # Start server in background
        RUST_LOG=info ./target/release/proximadb-server --config config/config.toml &
        SERVER_PID=$!
        
        # Wait for server to start
        wait_for_service "http://localhost:5678/health" 30
    fi
    
    # Check Python dependencies
    print_info "Checking Python dependencies..."
    if ! python3 -c "import numpy, requests" 2>/dev/null; then
        print_info "Installing Python dependencies..."
        pip install -r demo/requirements.txt
    fi
    
    # Install Python SDK
    print_info "Installing ProximaDB Python SDK..."
    cd clients/python
    pip install -e .
    cd ../..
    
    # Set PYTHONPATH for ProximaDB SDK
    export PYTHONPATH="$(pwd)/clients/python/src:$PYTHONPATH"
    
    # Run demo
    print_info "Running demo script..."
    cd demo
    python3 working_demo.py
    cd ..
    
    # Clean up server if we started it
    if [ ! -z "$SERVER_PID" ]; then
        print_info "Stopping ProximaDB server..."
        kill $SERVER_PID
    fi
}

# Function to run development demo
run_dev_demo() {
    print_info "Running ProximaDB demo in development mode..."
    
    # Start server in debug mode
    print_info "Starting ProximaDB server in debug mode..."
    RUST_LOG=debug cargo run --bin proximadb-server -- --config config/config.toml &
    SERVER_PID=$!
    
    # Wait for server
    wait_for_service "http://localhost:5678/health" 30
    
    # Run demo with development settings
    print_info "Running demo with development settings..."
    cd demo
    python3 demo.py --verbose --num-vectors 100
    cd ..
    
    # Clean up
    if [ ! -z "$SERVER_PID" ]; then
        print_info "Stopping server..."
        kill $SERVER_PID
    fi
}

# Function to show usage
show_usage() {
    echo "ProximaDB Demo Runner"
    echo ""
    echo "Usage: $0 [OPTION]"
    echo ""
    echo "Options:"
    echo "  docker     Run demo using Docker Compose (recommended)"
    echo "  local      Run demo locally (requires Rust and Python)"
    echo "  dev        Run demo in development mode with debug output"
    echo "  help       Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 docker    # Run complete demo environment with Docker"
    echo "  $0 local     # Run demo against local ProximaDB server"
    echo "  $0 dev       # Run demo with debug output and smaller dataset"
    echo ""
}

# Function to check prerequisites
check_prerequisites() {
    local mode=$1
    
    case $mode in
        "docker")
            if ! command_exists docker; then
                print_error "Docker not found. Please install Docker."
                exit 1
            fi
            if ! command_exists docker-compose; then
                print_error "docker-compose not found. Please install Docker Compose."
                exit 1
            fi
            ;;
        "local"|"dev")
            if ! command_exists cargo; then
                print_error "Cargo not found. Please install Rust."
                exit 1
            fi
            if ! command_exists python3; then
                print_error "Python 3 not found. Please install Python 3."
                exit 1
            fi
            if ! command_exists pip; then
                print_error "pip not found. Please install pip."
                exit 1
            fi
            ;;
    esac
}

# Main script logic
main() {
    local mode=${1:-"docker"}
    
    echo "🎭 ProximaDB Demo Runner"
    echo "======================="
    echo ""
    
    case $mode in
        "docker")
            check_prerequisites "docker"
            run_docker_demo
            ;;
        "local")
            check_prerequisites "local"
            run_local_demo
            ;;
        "dev")
            check_prerequisites "dev"
            run_dev_demo
            ;;
        "help"|"-h"|"--help")
            show_usage
            ;;
        *)
            print_error "Unknown option: $mode"
            echo ""
            show_usage
            exit 1
            ;;
    esac
    
    print_success "Demo runner completed!"
}

# Run main function
main "$@"
#!/bin/bash
# ProximaDB MVP Demo Docker Build Script

set -e

echo "🐳 Building ProximaDB MVP Demo Container"
echo "   Version: 0.1.4"
echo ""

# Configuration
IMAGE_NAME="proximadb:mvp-demo"
BUILD_ARGS=""

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --dev|--development)
            IMAGE_NAME="proximadb:dev"
            BUILD_ARGS="--target builder"
            echo "🔧 Building development image..."
            shift
            ;;
        --prod|--production)
            IMAGE_NAME="proximadb:production"
            BUILD_ARGS="--build-arg RUSTFLAGS='-C target-cpu=native -C opt-level=3 -C lto=fat'"
            echo "🚀 Building production-optimized image..."
            shift
            ;;
        --no-cache)
            BUILD_ARGS="$BUILD_ARGS --no-cache"
            echo "🔄 Building without cache..."
            shift
            ;;
        --tag=*)
            IMAGE_NAME="${1#*=}"
            echo "🏷️ Using custom tag: $IMAGE_NAME"
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --dev, --development     Build development image with build tools"
            echo "  --prod, --production     Build production-optimized image"
            echo "  --no-cache              Build without using cache"
            echo "  --tag=IMAGE_NAME        Use custom image name/tag"
            echo "  -h, --help              Show this help message"
            echo ""
            echo "Examples:"
            echo "  $0                      # Build standard MVP demo"
            echo "  $0 --dev                # Build development image"
            echo "  $0 --prod --no-cache    # Build production image from scratch"
            echo "  $0 --tag=my-proximadb  # Build with custom tag"
            exit 0
            ;;
        *)
            echo "❌ Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

echo "📦 Building image: $IMAGE_NAME"
if [ -n "$BUILD_ARGS" ]; then
    echo "🔧 Build arguments: $BUILD_ARGS"
fi
echo ""

# Check if Docker is running
if ! docker info >/dev/null 2>&1; then
    echo "❌ Docker is not running. Please start Docker and try again."
    exit 1
fi

# Build the image
echo "🏗️ Starting Docker build..."
start_time=$(date +%s)

if docker build $BUILD_ARGS -t "$IMAGE_NAME" .; then
    end_time=$(date +%s)
    build_duration=$((end_time - start_time))
    
    echo ""
    echo "✅ Build completed successfully!"
    echo "   Image: $IMAGE_NAME"
    echo "   Duration: ${build_duration}s"
    
    # Show image size
    image_size=$(docker images "$IMAGE_NAME" --format "table {{.Size}}" | tail -n 1)
    echo "   Size: $image_size"
    
    echo ""
    echo "🚀 Quick start commands:"
    echo "   # Run the demo"
    echo "   docker run -p 5678:5678 -p 5679:5679 -v proximadb-data:/data $IMAGE_NAME"
    echo ""
    echo "   # Use Docker Compose"
    echo "   docker-compose up"
    echo ""
    echo "   # Test the container"
    echo "   ./docker-demo-test.sh"
    
else
    echo ""
    echo "❌ Build failed!"
    exit 1
fi
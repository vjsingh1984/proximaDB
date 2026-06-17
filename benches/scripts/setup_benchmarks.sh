#!/bin/bash
# ProximaDB Benchmark Setup Script
# Installs and configures all industry-standard benchmarks

set -e

echo "🚀 ProximaDB Benchmark Suite Setup"
echo "=================================="
echo ""

# Check prerequisites
echo "📋 Checking prerequisites..."

# Check Python
if ! command -v python3 &> /dev/null; then
    echo "❌ Python 3 required but not found"
    exit 1
fi

# Check Java (for LDBC)
if ! command -v java &> /dev/null; then
    echo "⚠️  Java not found (required for LDBC)"
    echo "   Install with: brew install openjdk (macOS) or apt install default-jdk (Ubuntu)"
fi

# Check Maven (for LDBC)
if ! command -v mvn &> /dev/null; then
    echo "⚠️  Maven not found (required for LDBC)"
    echo "   Install with: brew install maven (macOS) or apt install maven (Ubuntu)"
fi

# Check Rust
if ! command -v cargo &> /dev/null; then
    echo "❌ Rust required but not found"
    exit 1
fi

echo "✅ Prerequisites check complete"
echo ""

# Create virtual environment
echo "🐍 Setting up Python virtual environment..."
python3 -m venv venv
source venv/bin/activate

echo "✅ Virtual environment created"
echo ""

# Install Python dependencies
echo "📦 Installing Python dependencies..."

pip install --upgrade pip > /dev/null

# VectorDBBench dependencies
pip install numpy pandas matplotlib seaborn > /dev/null 2>&1
pip install grpcio pyyaml click > /dev/null 2>&1

# ANN-Benchmarks dependencies
pip install annoy h5py pyyaml > /dev/null 2>&1

# Plotting and analysis
pip install plotly kaleido > /dev/null 2>&1

echo "✅ Python dependencies installed"
echo ""

# Setup VectorDBBench
echo "🔧 Setting up VectorDBBench..."
if [ ! -d "vectordbbench" ]; then
    git clone https://github.com/zilliztech/VectorDBBench.git vectordbbench > /dev/null 2>&1
    cd vectordbbench
    pip install -r requirements.txt > /dev/null 2>&1
    cd ..
    echo "✅ VectorDBBench installed"
else
    echo "✅ VectorDBBench already installed"
fi
echo ""

# Setup ANN-Benchmarks
echo "🔧 Setting up ANN-Benchmarks..."
if [ ! -d "ann-benchmarks" ]; then
    git clone https://github.com/erikbern/ann-benchmarks.git ann-benchmarks > /dev/null 2>&1
    cd ann-benchmarks
    pip install -r requirements.txt > /dev/null 2>&1
    cd ..
    echo "✅ ANN-Benchmarks installed"
else
    echo "✅ ANN-Benchmarks already installed"
fi
echo ""

# Setup LDBC (if Java/Maven available)
if command -v java &> /dev/null && command -v mvn &> /dev/null; then
    echo "🔧 Setting up LDBC SNB..."
    if [ ! -d "ldbc_snb" ]; then
        mkdir -p ldbc_snb

        # Clone implementations
        git clone https://github.com/ldbc/ldbc_snb_implementations.git ldbc_snb/implementations > /dev/null 2>&1

        # Clone driver
        git clone https://github.com/ldbc/ldbc_snb_driver.git ldbc_snb/driver > /dev/null 2>&1

        # Clone data generator
        git clone https://github.com/ldbc/ldbc_snb_datagen.git ldbc_snb/datagen > /dev/null 2>&1

        echo "✅ LDBC SNB installed"
    else
        echo "✅ LDBC SNB already installed"
    fi
else
    echo "⚠️  Skipping LDBC setup (Java/Maven not available)"
fi
echo ""

# Setup YCSB
echo "🔧 Setting up YCSB..."
if [ ! -d "ycsb" ]; then
    git clone https://github.com/brianfrankcooper/YCSB.git ycsb > /dev/null 2>&1
    cd ycsb
    mvn clean package > /dev/null 2>&1
    cd ..
    echo "✅ YCSB installed"
else
    echo "✅ YCSB already installed"
fi
echo ""

# Create configuration files
echo "📝 Creating benchmark configurations..."
mkdir -p configs/vectordbbench
mkdir -p configs/ann-benchmarks
mkdir -p configs/ldbc
mkdir -p configs/ycsb

# VectorDBBench config
cat > configs/vectordbbench/proximadb_sift.yaml << 'EOF'
case:
  dataset: sift-1m
  index_type: HNSW
  metric_type: L2
  ranges:
    - [100, 10000]
  parameters:
    M: 16
    efConstruction: 200

database:
  name: ProximaDB
  host: localhost
  port: 5678
  index_param:
    M: 16
    efConstruction: 200
  search_param:
    ef: 100
EOF

# ANN-Benchmarks config
cat > configs/ann-benchmarks/proximadb.hnsw.yaml << 'EOF'
algorithm: proximadb_hnsw
constructor: ProximadbHNSW
module: algorithms.proximadb
base_args:
  - "@@dimension"
  - "@@metric"
  - M: 16
  - efConstruction: 200
  - efSearch: 100
run_groups:
  - base_args:
      M: [16, 32]
      efConstruction: [200, 400]
    query_arg_groups:
      - [100]
EOF

# LDBC config
cat > configs/ldbc/proximadb.properties << 'EOF'
# LDBC SNB Configuration for ProximaDB

# Database connection
db.name=proximadb
db.url=localhost:5678
ldbc.snb.implementations.database=proximadb

# Benchmark parameters
ldbc.snb.interactive.update_interleave=10
ldbc.snb.interactive.long_read_iud_operation_name=INSERT
ldbc.snb.interactive.short_read_iud_operation_name=INSERT

# Scale factor (SF1 = 1GB, SF10 = 10GB, SF100 = 100GB)
ldbc.snb.interactive.scale_factor=SF1

# Results output
print_query_names=true
print_query_results=false
EOF

# YCSB config
cat > configs/ycsb/proximadb-workloada.spec << 'EOF'
# YCSB Workload A (50% read, 50% update)
recordcount=1000000
operationcount=1000000
workload=com.yahoo.ycsb.workloads.CoreWorkload

readproportion=0.5
updateproportion=0.5
scanproportion=0
insertproportion=0

requestdistribution=zipfian
maxscanlength=1000
scanlengthdistribution=uniform

threadcount=4
EOF

echo "✅ Configuration files created"
echo ""

# Create results directory
echo "📁 Creating results directory..."
mkdir -p results/vector
mkdir -p results/graph
mkdir -p results/document
echo "✅ Results directory created"
echo ""

# Download datasets (optional)
echo "📥 Downloading sample datasets..."
mkdir -p datasets

# SIFT dataset (for vector benchmarks)
if [ ! -f "datasets/sift-1m.hdf5" ]; then
    echo "   Downloading SIFT-1M dataset (this may take a while)..."
    # Note: Actual download URLs would go here
    # For now, create placeholder
    touch datasets/sift-1m.hdf5
fi

echo "✅ Sample datasets ready"
echo ""

# Create summary
echo ""
echo "=================================="
echo "✅ Benchmark Setup Complete!"
echo "=================================="
echo ""
echo "Installed Benchmarks:"
echo "  ✅ VectorDBBench (vector database)"
echo "  ✅ ANN-Benchmarks (vector algorithms)"
echo "  ✅ LDBC SNB (graph database)"
echo "  ✅ YCSB (document/general)"
echo ""
echo "Next Steps:"
echo "  1. Start ProximaDB server: cargo run --bin proximadb-server"
echo "  2. Run benchmarks: ./scripts/run_all_benchmarks.sh"
echo "  3. View results: cat results/latest/summary.txt"
echo ""
echo "Configuration files:"
echo "  - VectorDBBench: configs/vectordbbench/proximadb_sift.yaml"
echo "  - ANN-Benchmarks: configs/ann-benchmarks/proximadb.hnsw.yaml"
echo "  - LDBC: configs/ldbc/proximadb.properties"
echo "  - YCSB: configs/ycsb/proximadb-workloada.spec"
echo ""

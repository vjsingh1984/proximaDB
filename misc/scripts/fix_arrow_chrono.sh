#!/bin/bash

echo "🔧 Fixing Arrow/Chrono quarter method conflict..."

# Create a patch for the Arrow crate to fix the quarter method ambiguity
# We'll use a cargo patch to override the arrow-arith dependency

mkdir -p ~/.cargo
cat > ~/.cargo/config.toml << 'EOF'
[patch.crates-io]
arrow-arith = { path = "/tmp/arrow-arith-patched" }
EOF

# Download and patch arrow-arith
echo "📦 Downloading and patching arrow-arith..."
cd /tmp
cargo download arrow-arith --version 44.0.0 || {
    # Alternative: clone from git
    git clone https://github.com/apache/arrow-rs.git --depth 1 --branch 44.0.0
    cp -r arrow-rs/arrow-arith ./arrow-arith-patched
}

# If cargo download worked
if [ -d "arrow-arith-44.0.0" ]; then
    cp -r arrow-arith-44.0.0 arrow-arith-patched
fi

# Apply the fix to temporal.rs
if [ -f "arrow-arith-patched/src/temporal.rs" ]; then
    echo "🔧 Patching temporal.rs..."
    
    # Fix the quarter method calls by explicitly qualifying them
    sed -i 's/|t| t\.quarter() as i32/|t| chrono::Datelike::quarter(\&t) as i32/g' arrow-arith-patched/src/temporal.rs
    
    echo "✅ Applied quarter method disambiguation patch"
    
    # Show what we changed
    echo "📋 Changes made:"
    grep -n "quarter" arrow-arith-patched/src/temporal.rs || true
    
else
    echo "❌ Could not find temporal.rs file to patch"
fi

echo "🔙 Returning to workspace..."
cd /workspace
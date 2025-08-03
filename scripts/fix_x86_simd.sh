#!/bin/bash

# Fix x86 SIMD feature detection for ARM64 compatibility

echo "🔧 Fixing x86 SIMD feature detection calls for ARM64 compatibility..."

# Backup the original file
cp /workspace/src/compute/distance.rs /workspace/src/compute/distance.rs.backup

# Use sed to wrap x86 feature detection in target_arch guards
# This is a bit complex, so let's do it in Python for better control

python3 << 'EOF'
import re

# Read the file
with open('/workspace/src/compute/distance.rs', 'r') as f:
    content = f.read()

# Pattern to match if is_x86_feature_detected!(...) blocks
pattern = r'(\s*)(if\s+is_x86_feature_detected!\([^)]+\)(?:\s*&&\s*is_x86_feature_detected!\([^)]+\))?)\s*\{\s*\n(\s*unsafe\s*\{\s*[^}]+\}\s*\n\s*\}\s*else\s*\{\s*\n\s*[^}]+\n\s*\})'

def replace_x86_detection(match):
    indent = match.group(1)
    condition = match.group(2)
    block = match.group(3)
    
    return f'''{indent}#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
{indent}{{
{indent}    {condition} {{
{block}
{indent}}}
{indent}#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
{indent}{{
{indent}    self.{("cosine_similarity" if "cosine" in block else "euclidean_distance" if "euclidean" in block else "dot_product")}_scalar(a, b)
{indent}}}'''

# Apply the replacement
new_content = re.sub(pattern, replace_x86_detection, content, flags=re.MULTILINE | re.DOTALL)

# Write back the file
with open('/workspace/src/compute/distance.rs', 'w') as f:
    f.write(new_content)

print("✅ Fixed x86 SIMD feature detection")
EOF

echo "🔍 Checking for remaining x86 feature detection calls..."
remaining=$(grep -c "is_x86_feature_detected" /workspace/src/compute/distance.rs || echo "0")

if [ "$remaining" -gt "0" ]; then
    echo "⚠️  Still have $remaining calls to fix manually"
    grep -n "is_x86_feature_detected" /workspace/src/compute/distance.rs
else
    echo "✅ All x86 feature detection calls fixed!"
fi
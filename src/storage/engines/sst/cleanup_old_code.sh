#!/bin/bash

# Remove old SstRecord code from line 362 to 420
sed -i '362,420d' /home/vsingh/code/proximaDB/src/storage/engines/sst/mod.rs

echo "Cleaned up old SstRecord conversion code"
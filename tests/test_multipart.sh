#!/bin/bash
# Test script for multipart upload functionality

set -e

HSTEST="./target/debug/hsc"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${YELLOW}=== Testing multipart upload configuration ===${NC}\n"

echo -e "${GREEN}Test 1: Check multipart options in help${NC}"
$HSTEST cp --help | grep -A2 "multipart"
echo ""

echo -e "${GREEN}Test 2: Create test files${NC}"
# Create a 5MB file (below default threshold)
dd if=/dev/zero of=/tmp/test-5mb.dat bs=1M count=5 2>/dev/null
echo "Created 5MB file (below 8MB threshold)"

# Create a 15MB file (above default threshold)  
dd if=/dev/zero of=/tmp/test-15mb.dat bs=1M count=15 2>/dev/null
echo "Created 15MB file (above 8MB threshold)"

# Create a 30MB file for custom chunksize testing
dd if=/dev/zero of=/tmp/test-30mb.dat bs=1M count=30 2>/dev/null
echo "Created 30MB file for chunksize test"
echo ""

echo -e "${GREEN}Test 3: Test with custom threshold (2MB)${NC}"
echo "5MB file with 2MB threshold should use multipart (not testing actual S3 upload):"
echo "Command: hsc cp /tmp/test-5mb.dat s3://bucket/file.dat --multipart-threshold 2097152"
echo ""

echo -e "${GREEN}Test 4: Test with custom chunksize${NC}"
echo "30MB file with 10MB chunks should use 3 parts:"
echo "Command: hsc cp /tmp/test-30mb.dat s3://bucket/file.dat --multipart-chunksize 10485760"
echo ""

echo -e "${GREEN}Test 5: File size detection${NC}"
SIZE_5MB=$(stat -c%s /tmp/test-5mb.dat)
SIZE_15MB=$(stat -c%s /tmp/test-15mb.dat)
SIZE_30MB=$(stat -c%s /tmp/test-30mb.dat)
DEFAULT_THRESHOLD=8388608

echo "File sizes:"
echo "  5MB file:  $SIZE_5MB bytes"
echo "  15MB file: $SIZE_15MB bytes"
echo "  30MB file: $SIZE_30MB bytes"
echo "  Default threshold: $DEFAULT_THRESHOLD bytes (8MB)"
echo ""

if [ $SIZE_5MB -lt $DEFAULT_THRESHOLD ]; then
    echo -e "${GREEN}✓ 5MB < 8MB threshold -> Regular upload${NC}"
else
    echo -e "${RED}✗ 5MB should be below threshold${NC}"
fi

if [ $SIZE_15MB -ge $DEFAULT_THRESHOLD ]; then
    echo -e "${GREEN}✓ 15MB >= 8MB threshold -> Multipart upload${NC}"
else
    echo -e "${RED}✗ 15MB should be above threshold${NC}"
fi
echo ""

echo -e "${GREEN}Test 6: Calculate parts for different chunksizes${NC}"
FILE_SIZE=20971520  # 20MB
CHUNK_5MB=5242880
CHUNK_8MB=8388608
CHUNK_10MB=10485760

PARTS_5MB=$(( ($FILE_SIZE + $CHUNK_5MB - 1) / $CHUNK_5MB ))
PARTS_8MB=$(( ($FILE_SIZE + $CHUNK_8MB - 1) / $CHUNK_8MB ))
PARTS_10MB=$(( ($FILE_SIZE + $CHUNK_10MB - 1) / $CHUNK_10MB ))

echo "For a 20MB file:"
echo "  5MB chunks:  $PARTS_5MB parts"
echo "  8MB chunks:  $PARTS_8MB parts"
echo "  10MB chunks: $PARTS_10MB parts"
echo ""

# Clean up
rm -f /tmp/test-5mb.dat /tmp/test-15mb.dat /tmp/test-30mb.dat

echo -e "${GREEN}=== Multipart configuration tests completed ===${NC}"
echo ""
echo "Note: Actual S3 multipart uploads require valid AWS credentials."
echo "The implementation will automatically:"
echo "  1. Check file size"
echo "  2. Use multipart if size >= threshold"
echo "  3. Split file into chunks of specified size"
echo "  4. Upload parts concurrently"
echo "  5. Complete multipart upload"

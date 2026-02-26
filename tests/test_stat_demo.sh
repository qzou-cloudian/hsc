#!/bin/bash
# Test script for enhanced stat command with recursive and checksum features

set -e

HSTEST="./target/debug/hsc"
TEST_DIR="/tmp/hsc-stat-demo"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}=== Testing enhanced stat command ===${NC}\n"

# Clean up any previous test data
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR/subdir1/subdir2"

# Create test files with known content
echo "Hello World" > "$TEST_DIR/file1.txt"
echo "Test content for checksums" > "$TEST_DIR/file2.txt"
echo "Nested file content" > "$TEST_DIR/subdir1/file3.txt"
echo "Deep nested file" > "$TEST_DIR/subdir1/subdir2/file4.txt"

echo -e "${GREEN}Test 1: Basic stat on single file (with ETag)${NC}"
$HSTEST stat "$TEST_DIR/file1.txt"
echo ""

echo -e "${GREEN}Test 2: Stat with SHA256 checksum${NC}"
$HSTEST stat "$TEST_DIR/file1.txt" --checksum-mode ENABLED --checksum-algorithm SHA256
echo ""

echo -e "${GREEN}Test 3: Stat with SHA1 checksum${NC}"
$HSTEST stat "$TEST_DIR/file2.txt" --checksum-mode enabled --checksum-algorithm SHA1
echo ""

echo -e "${GREEN}Test 4: Stat with CRC32 checksum${NC}"
$HSTEST stat "$TEST_DIR/file2.txt" --checksum-mode ENABLED --checksum-algorithm CRC32
echo ""

echo -e "${GREEN}Test 5: Stat on directory (non-recursive)${NC}"
$HSTEST stat "$TEST_DIR"
echo ""

echo -e "${GREEN}Test 6: Recursive stat (no checksums)${NC}"
$HSTEST stat "$TEST_DIR" --recursive
echo ""

echo -e "${GREEN}Test 7: Recursive stat with SHA1 checksums${NC}"
$HSTEST stat "$TEST_DIR" --recursive --checksum-mode ENABLED --checksum-algorithm SHA1 | head -40
echo ""

echo -e "${GREEN}Test 8: Stat on nested directory (recursive)${NC}"
$HSTEST stat "$TEST_DIR/subdir1" --recursive
echo ""

# Verify ETag calculation (MD5)
echo -e "${GREEN}Test 9: Verify ETag matches MD5${NC}"
EXPECTED_MD5=$(md5sum "$TEST_DIR/file1.txt" | awk '{print $1}')
ETAG=$($HSTEST stat "$TEST_DIR/file1.txt" | grep "ETag" | awk '{print $3}' | tr -d '"')
echo "Expected MD5: $EXPECTED_MD5"
echo "Calculated ETag: $ETAG"
if [ "$EXPECTED_MD5" = "$ETAG" ]; then
    echo -e "${GREEN}✓ ETag matches MD5${NC}"
else
    echo "✗ ETag does NOT match MD5"
fi
echo ""

# Verify SHA256 calculation
echo -e "${GREEN}Test 10: Verify SHA256 checksum${NC}"
EXPECTED_SHA256=$(sha256sum "$TEST_DIR/file1.txt" | awk '{print $1}')
SHA256=$($HSTEST stat "$TEST_DIR/file1.txt" --checksum-mode ENABLED --checksum-algorithm SHA256 | grep "SHA256" | awk '{print $3}')
echo "Expected SHA256: $EXPECTED_SHA256"
echo "Calculated SHA256: $SHA256"
if [ "$EXPECTED_SHA256" = "$SHA256" ]; then
    echo -e "${GREEN}✓ SHA256 matches${NC}"
else
    echo "✗ SHA256 does NOT match"
fi
echo ""

# Clean up
rm -rf "$TEST_DIR"

echo -e "${GREEN}=== All stat tests completed successfully ===${NC}"


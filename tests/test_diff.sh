#!/bin/bash
# Test script for diff command

set -e

HSTEST="./target/debug/hsc"
TEST_DIR="/tmp/hsc-diff-test"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}=== Testing diff command ===${NC}\n"

# Clean up any previous test data
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR"/{source,dest}

# Setup test data
echo "Test 1: Create identical files"
echo "same content" > "$TEST_DIR/source/same.txt"
echo "same content" > "$TEST_DIR/dest/same.txt"

echo "Test 2: Create files only in source"
echo "only in source" > "$TEST_DIR/source/source-only.txt"
mkdir -p "$TEST_DIR/source/subdir"
echo "nested source" > "$TEST_DIR/source/subdir/nested.txt"

echo "Test 3: Create files only in dest"
echo "only in dest" > "$TEST_DIR/dest/dest-only.txt"

echo "Test 4: Create files with different sizes"
echo "short" > "$TEST_DIR/source/different-size.txt"
echo "much longer content here" > "$TEST_DIR/dest/different-size.txt"

echo "Test 5: Create files with same size but different content"
echo "content A exactly!!" > "$TEST_DIR/source/different-content.txt"
echo "content B exactly!!" > "$TEST_DIR/dest/different-content.txt"

echo "Test 6: Create files for filter testing"
echo "log file" > "$TEST_DIR/source/app.log"
echo "log file" > "$TEST_DIR/dest/app.log"
echo "text file" > "$TEST_DIR/source/doc.txt"

echo ""
echo -e "${GREEN}Test 1: Basic diff (no content comparison)${NC}"
$HSTEST diff "$TEST_DIR/source" "$TEST_DIR/dest"
echo ""

echo -e "${GREEN}Test 2: Diff with content comparison${NC}"
$HSTEST diff "$TEST_DIR/source" "$TEST_DIR/dest" --compare-content
echo ""

echo -e "${GREEN}Test 3: Diff with include filter (*.txt only)${NC}"
$HSTEST diff "$TEST_DIR/source" "$TEST_DIR/dest" --include "*.txt"
echo ""

echo -e "${GREEN}Test 4: Diff with exclude filter (exclude *.log)${NC}"
$HSTEST diff "$TEST_DIR/source" "$TEST_DIR/dest" --exclude "*.log"
echo ""

echo -e "${GREEN}Test 5: Diff identical directories${NC}"
rm -rf "$TEST_DIR"/{source,dest}
mkdir -p "$TEST_DIR"/{source,dest}
echo "same" > "$TEST_DIR/source/file.txt"
echo "same" > "$TEST_DIR/dest/file.txt"
$HSTEST diff "$TEST_DIR/source" "$TEST_DIR/dest"
echo ""

echo -e "${GREEN}Test 6: Verify content differs detection${NC}"
echo "content1" > "$TEST_DIR/source/test1.txt"
echo "content2" > "$TEST_DIR/dest/test1.txt"
RESULT=$($HSTEST diff "$TEST_DIR/source" "$TEST_DIR/dest" --compare-content | grep "Content differs")
if [ -n "$RESULT" ]; then
    echo -e "${GREEN}✓ Content difference detected correctly${NC}"
else
    echo "✗ Content difference NOT detected"
fi
echo ""

# Clean up
rm -rf "$TEST_DIR"

echo -e "${GREEN}=== All diff tests completed successfully ===${NC}"

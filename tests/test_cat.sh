#!/bin/bash
# Test script for cat command

set -e

HSTEST="./target/debug/hsc"
TEST_FILE="/tmp/hsc-cat-test.txt"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${YELLOW}=== Testing cat command ===${NC}\n"

# Create test file with known content
cat > "$TEST_FILE" << 'EOF'
Line 1: The quick brown fox jumps over the lazy dog.
Line 2: Pack my box with five dozen liquor jugs.
Line 3: How vexingly quick daft zebras jump!
Line 4: The five boxing wizards jump quickly.
EOF

FILE_SIZE=$(wc -c < "$TEST_FILE")
echo "Test file created: $FILE_SIZE bytes"
echo ""

echo -e "${GREEN}Test 1: Basic cat (entire file)${NC}"
RESULT=$($HSTEST cat "$TEST_FILE")
EXPECTED=$(cat "$TEST_FILE")
if [ "$RESULT" = "$EXPECTED" ]; then
    echo -e "${GREEN}✓ Output matches expected${NC}"
else
    echo -e "${RED}✗ Output does not match${NC}"
fi
echo ""

echo -e "${GREEN}Test 2: Cat with byte range (first 10 bytes)${NC}"
RESULT=$($HSTEST cat "$TEST_FILE" --range "0-9")
EXPECTED=$(head -c 10 "$TEST_FILE")
if [ "$RESULT" = "$EXPECTED" ]; then
    echo -e "${GREEN}✓ Range output correct: '$RESULT'${NC}"
else
    echo -e "${RED}✗ Range output incorrect${NC}"
fi
echo ""

echo -e "${GREEN}Test 3: Cat with bytes= prefix range${NC}"
RESULT=$($HSTEST cat "$TEST_FILE" --range "bytes=0-9")
EXPECTED=$(head -c 10 "$TEST_FILE")
if [ "$RESULT" = "$EXPECTED" ]; then
    echo -e "${GREEN}✓ bytes= prefix works${NC}"
else
    echo -e "${RED}✗ bytes= prefix failed${NC}"
fi
echo ""

echo -e "${GREEN}Test 4: Cat with offset only (from byte 50)${NC}"
RESULT=$($HSTEST cat "$TEST_FILE" --offset 50)
EXPECTED=$(tail -c +51 "$TEST_FILE")  # tail uses 1-based indexing
if [ "$RESULT" = "$EXPECTED" ]; then
    echo -e "${GREEN}✓ Offset-only output correct${NC}"
else
    echo -e "${RED}✗ Offset-only output incorrect${NC}"
fi
echo ""

echo -e "${GREEN}Test 5: Cat with offset and size${NC}"
RESULT=$($HSTEST cat "$TEST_FILE" --offset 20 --size 15)
EXPECTED=$(dd if="$TEST_FILE" bs=1 skip=20 count=15 2>/dev/null)
if [ "$RESULT" = "$EXPECTED" ]; then
    echo -e "${GREEN}✓ Offset+size output correct: '$RESULT'${NC}"
else
    echo -e "${RED}✗ Offset+size output incorrect${NC}"
fi
echo ""

echo -e "${GREEN}Test 6: Cat with open-ended range${NC}"
RESULT=$($HSTEST cat "$TEST_FILE" --range "100-")
EXPECTED=$(tail -c +101 "$TEST_FILE")
if [ "$RESULT" = "$EXPECTED" ]; then
    echo -e "${GREEN}✓ Open-ended range works${NC}"
else
    echo -e "${RED}✗ Open-ended range failed${NC}"
fi
echo ""

echo -e "${GREEN}Test 7: Error handling - conflicting options${NC}"
ERROR=$($HSTEST cat "$TEST_FILE" --range "0-10" --offset 5 2>&1 || true)
if echo "$ERROR" | grep -q "Cannot specify both"; then
    echo -e "${GREEN}✓ Correctly rejects conflicting options${NC}"
else
    echo -e "${RED}✗ Should reject conflicting options${NC}"
fi
echo ""

echo -e "${GREEN}Test 8: Error handling - file not found${NC}"
ERROR=$($HSTEST cat "/tmp/nonexistent-file-12345.txt" 2>&1 || true)
if echo "$ERROR" | grep -q "does not exist"; then
    echo -e "${GREEN}✓ Correctly reports missing file${NC}"
else
    echo -e "${RED}✗ Should report missing file${NC}"
fi
echo ""

echo -e "${GREEN}Test 9: Error handling - directory instead of file${NC}"
ERROR=$($HSTEST cat "/tmp" 2>&1 || true)
if echo "$ERROR" | grep -q "not a file"; then
    echo -e "${GREEN}✓ Correctly rejects directory${NC}"
else
    echo -e "${RED}✗ Should reject directory${NC}"
fi
echo ""

echo -e "${GREEN}Test 10: Large file handling${NC}"
LARGE_FILE="/tmp/hsc-large.txt"
# Create 1MB file
dd if=/dev/zero of="$LARGE_FILE" bs=1024 count=1024 2>/dev/null
RESULT_SIZE=$($HSTEST cat "$LARGE_FILE" | wc -c)
EXPECTED_SIZE=$(wc -c < "$LARGE_FILE")
if [ "$RESULT_SIZE" -eq "$EXPECTED_SIZE" ]; then
    echo -e "${GREEN}✓ Large file handled correctly (1MB)${NC}"
else
    echo -e "${RED}✗ Large file size mismatch${NC}"
fi
rm -f "$LARGE_FILE"
echo ""

echo -e "${GREEN}Test 11: Pipe to other commands${NC}"
RESULT=$($HSTEST cat "$TEST_FILE" | grep "quick" | wc -l)
if [ "$RESULT" -eq 3 ]; then
    echo -e "${GREEN}✓ Piping works correctly${NC}"
else
    echo -e "${RED}✗ Piping failed (expected 3, got $RESULT)${NC}"
fi
echo ""

echo -e "${GREEN}Test 12: Binary file handling${NC}"
BINARY_FILE="/tmp/hsc-binary.bin"
# Create binary file with specific bytes
printf '\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09' > "$BINARY_FILE"
RESULT_SIZE=$($HSTEST cat "$BINARY_FILE" | wc -c)
if [ "$RESULT_SIZE" -eq 10 ]; then
    echo -e "${GREEN}✓ Binary file handled correctly${NC}"
else
    echo -e "${RED}✗ Binary file size mismatch${NC}"
fi
rm -f "$BINARY_FILE"
echo ""

# Clean up
rm -f "$TEST_FILE"

echo -e "${GREEN}=== All cat tests completed successfully ===${NC}"

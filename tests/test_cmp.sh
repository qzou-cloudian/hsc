#!/bin/bash
# Test script for cmp command
# Tests local/local, s3/s3, local/s3, range, offset/size, and error cases

set +e

BINARY="./target/debug/hsc"
TEST_DIR="/tmp/hsc-cmp-test-$$"
BUCKET_NAME=""

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

PASS=0
FAIL=0

pass() { echo -e "${GREEN}✓ $1${NC}"; PASS=$((PASS + 1)); }
fail() { echo -e "${RED}✗ $1${NC}"; FAIL=$((FAIL + 1)); }

echo -e "${YELLOW}=== Testing cmp command ===${NC}"
echo ""

# ── Setup ──────────────────────────────────────────────────────────────────────

mkdir -p "$TEST_DIR"

# Create test files
printf 'ABCDEFGHIJ0123456789abcdefghijklmnopqrstuvwxyz' > "$TEST_DIR/file_a.bin"
cp "$TEST_DIR/file_a.bin" "$TEST_DIR/file_same.bin"

# file_b differs at byte 11 (0-based index 10 → 1-based 11): 'A' vs '0'
printf 'ABCDEFGHIJ_123456789abcdefghijklmnopqrstuvwxyz' > "$TEST_DIR/file_b.bin"

# Short file (truncated at byte 10)
printf 'ABCDEFGHIJ' > "$TEST_DIR/file_short.bin"

# Text file for line-number reporting
printf 'line1\nline2\nline3 DIFF\nline4\n' > "$TEST_DIR/text_a.txt"
printf 'line1\nline2\nline3 same\nline4\n' > "$TEST_DIR/text_b.txt"

echo "Test files created in $TEST_DIR"
echo ""

# ── Local/Local tests ─────────────────────────────────────────────────────────

echo -e "${YELLOW}--- Local/Local tests ---${NC}"

echo "Test 1: Identical files → exit 0"
if $BINARY cmp "$TEST_DIR/file_a.bin" "$TEST_DIR/file_same.bin" 2>/dev/null; then
    pass "Identical files: exit 0"
else
    fail "Identical files should exit 0"
fi

echo "Test 2: Differing files → exit 1"
if $BINARY cmp "$TEST_DIR/file_a.bin" "$TEST_DIR/file_b.bin" 2>/dev/null; then
    fail "Differing files should exit 1"
else
    pass "Differing files: exit 1"
fi

echo "Test 3: Differing files → correct byte and line reported"
OUTPUT=$($BINARY cmp "$TEST_DIR/file_a.bin" "$TEST_DIR/file_b.bin" 2>&1 || true)
if echo "$OUTPUT" | grep -q "byte 11"; then
    pass "Reports correct byte position (11)"
else
    fail "Expected 'byte 11' in output, got: $OUTPUT"
fi

echo "Test 4: Different-size files → EOF reported"
OUTPUT=$($BINARY cmp "$TEST_DIR/file_a.bin" "$TEST_DIR/file_short.bin" 2>&1 || true)
if echo "$OUTPUT" | grep -q "EOF"; then
    pass "Different-size files: EOF reported"
else
    fail "Expected EOF message, got: $OUTPUT"
fi

echo "Test 5: Text file diff → line number reported"
OUTPUT=$($BINARY cmp "$TEST_DIR/text_a.txt" "$TEST_DIR/text_b.txt" 2>&1 || true)
if echo "$OUTPUT" | grep -q "line 3"; then
    pass "Reports correct line number (3)"
else
    fail "Expected 'line 3' in output, got: $OUTPUT"
fi

echo ""

# ── Range tests ───────────────────────────────────────────────────────────────

echo -e "${YELLOW}--- Range/Offset tests ---${NC}"

echo "Test 6: --range of identical bytes → exit 0"
# bytes 0-9 are 'ABCDEFGHIJ' in both files (file_a vs file_b differ at byte 10)
if $BINARY cmp --range "0-9" "$TEST_DIR/file_a.bin" "$TEST_DIR/file_b.bin" 2>/dev/null; then
    pass "Range 0-9 identical: exit 0"
else
    fail "Range 0-9 should be identical"
fi

echo "Test 7: --range covering differing byte → exit 1"
if $BINARY cmp --range "0-15" "$TEST_DIR/file_a.bin" "$TEST_DIR/file_b.bin" 2>/dev/null; then
    fail "Range 0-15 should detect difference"
else
    pass "Range 0-15 detects difference: exit 1"
fi

echo "Test 8: --offset skips differing byte → exit 0"
# Skip first 11 bytes (diff is at byte 11), compare remainder
# file_a[11..] = '123456789abc...' and file_b[11..] = '123456789abc...' (they match after byte 11)
if $BINARY cmp --offset 11 "$TEST_DIR/file_a.bin" "$TEST_DIR/file_b.bin" 2>/dev/null; then
    pass "Offset 11 skips diff: exit 0"
else
    fail "Offset 11 should skip the differing byte"
fi

echo "Test 9: --offset and --size (identical window) → exit 0"
if $BINARY cmp --offset 20 --size 10 "$TEST_DIR/file_a.bin" "$TEST_DIR/file_b.bin" 2>/dev/null; then
    pass "Offset+size identical window: exit 0"
else
    fail "Offset 20 size 10 should be identical"
fi

echo "Test 10: bytes= prefix in --range"
if $BINARY cmp --range "bytes=0-9" "$TEST_DIR/file_a.bin" "$TEST_DIR/file_same.bin" 2>/dev/null; then
    pass "bytes= prefix accepted: exit 0"
else
    fail "bytes= prefix should work"
fi

echo "Test 11: Open-ended range (start-)"
if $BINARY cmp --range "0-" "$TEST_DIR/file_a.bin" "$TEST_DIR/file_same.bin" 2>/dev/null; then
    pass "Open-ended range: exit 0"
else
    fail "Open-ended range should work"
fi

echo ""

# ── Error handling ────────────────────────────────────────────────────────────

echo -e "${YELLOW}--- Error handling ---${NC}"

echo "Test 12: --range and --offset conflict"
OUTPUT=$($BINARY cmp --range "0-9" --offset 5 "$TEST_DIR/file_a.bin" "$TEST_DIR/file_same.bin" 2>&1 || true)
if echo "$OUTPUT" | grep -qi "cannot specify both"; then
    pass "Correctly rejects --range + --offset"
else
    fail "Should reject conflicting --range and --offset: $OUTPUT"
fi

echo "Test 13: Missing file"
OUTPUT=$($BINARY cmp "$TEST_DIR/nonexistent.bin" "$TEST_DIR/file_a.bin" 2>&1 || true)
if echo "$OUTPUT" | grep -qiE "error|cannot access|No such"; then
    pass "Missing file: error reported"
else
    fail "Should report error for missing file: $OUTPUT"
fi

echo "Test 14: Same file compared to itself"
if $BINARY cmp "$TEST_DIR/file_a.bin" "$TEST_DIR/file_a.bin" 2>/dev/null; then
    pass "Same file twice: exit 0"
else
    fail "Same file twice should be identical"
fi

echo ""

# ── S3 tests (optional, requires AWS credentials) ─────────────────────────────

if ! $BINARY ls 2>/dev/null | grep -q "."; then
    echo -e "${YELLOW}Skipping S3 tests (no S3 access)${NC}"
else
    echo -e "${YELLOW}--- S3 tests ---${NC}"
    BUCKET_NAME="hsc-cmp-test-$$"

    $BINARY mb "s3://$BUCKET_NAME" 2>/dev/null

    if ! $BINARY cp "$TEST_DIR/file_a.bin" "s3://$BUCKET_NAME/file_a.bin" 2>/dev/null ||
       ! $BINARY cp "$TEST_DIR/file_same.bin" "s3://$BUCKET_NAME/file_same.bin" 2>/dev/null ||
       ! $BINARY cp "$TEST_DIR/file_b.bin" "s3://$BUCKET_NAME/file_b.bin" 2>/dev/null; then
        echo -e "${YELLOW}Skipping S3 tests (upload failed — transient error?)${NC}"
        $BINARY rb "s3://$BUCKET_NAME" 2>/dev/null || true
    else

    echo "Test 15: S3/S3 identical"
    if $BINARY cmp "s3://$BUCKET_NAME/file_a.bin" "s3://$BUCKET_NAME/file_same.bin" 2>/dev/null; then
        pass "S3/S3 identical: exit 0"
    else
        fail "S3/S3 identical files should match"
    fi

    echo "Test 16: S3/S3 different"
    if $BINARY cmp "s3://$BUCKET_NAME/file_a.bin" "s3://$BUCKET_NAME/file_b.bin" 2>/dev/null; then
        fail "S3/S3 differing files should exit 1"
    else
        pass "S3/S3 different: exit 1"
    fi

    echo "Test 17: Local vs S3 identical"
    if $BINARY cmp "$TEST_DIR/file_a.bin" "s3://$BUCKET_NAME/file_a.bin" 2>/dev/null; then
        pass "Local/S3 identical: exit 0"
    else
        fail "Local vs S3 identical should match"
    fi

    echo "Test 18: Local vs S3 different"
    if $BINARY cmp "$TEST_DIR/file_a.bin" "s3://$BUCKET_NAME/file_b.bin" 2>/dev/null; then
        fail "Local vs S3 differing should exit 1"
    else
        pass "Local/S3 different: exit 1"
    fi

    echo "Test 19: S3/S3 with --range (identical window)"
    if $BINARY cmp --range "0-9" "s3://$BUCKET_NAME/file_a.bin" "s3://$BUCKET_NAME/file_b.bin" 2>/dev/null; then
        pass "S3/S3 range 0-9 identical: exit 0"
    else
        fail "S3/S3 range 0-9 should be identical"
    fi

    echo "Test 20: S3 vs local with --offset"
    if $BINARY cmp --offset 11 "s3://$BUCKET_NAME/file_a.bin" "$TEST_DIR/file_b.bin" 2>/dev/null; then
        pass "S3/local offset 11 identical: exit 0"
    else
        fail "S3/local offset 11 should be identical"
    fi

    # Cleanup S3
    $BINARY rm "s3://$BUCKET_NAME/file_a.bin" 2>/dev/null
    $BINARY rm "s3://$BUCKET_NAME/file_same.bin" 2>/dev/null
    $BINARY rm "s3://$BUCKET_NAME/file_b.bin" 2>/dev/null
    $BINARY rb "s3://$BUCKET_NAME" 2>/dev/null
    fi  # end upload-check
fi  # end S3-access-check

# ── Cleanup & summary ─────────────────────────────────────────────────────────

rm -rf "$TEST_DIR"

echo ""
echo "========================================="
echo -e "${GREEN}Passed: $PASS${NC}  ${RED}Failed: $FAIL${NC}"
echo "========================================="

[ "$FAIL" -eq 0 ]

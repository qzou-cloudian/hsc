#!/bin/bash
# Local-only coverage for JSON/reporting commands and utility commands.

set +e

BINARY="./target/debug/hsc"
TEST_DIR="/tmp/hsc-json-utils-$$"
PASS=0
FAIL=0

pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1"; FAIL=$((FAIL + 1)); }

mkdir -p "$TEST_DIR"/{a,b}
printf 'alpha\nbeta\ngamma\n' > "$TEST_DIR/a/same.txt"
cp "$TEST_DIR/a/same.txt" "$TEST_DIR/b/same.txt"
printf 'alpha\nbeta\nDIFF\n' > "$TEST_DIR/b/diff.txt"
printf 'only source\n' > "$TEST_DIR/a/source-only.txt"

OUT=$($BINARY exists "$TEST_DIR/a/same.txt" 2>/dev/null)
if [ "$OUT" = "true" ]; then pass "exists returns true for local file"; else fail "exists local true"; fi

$BINARY exists "$TEST_DIR/missing.txt" >/dev/null 2>&1
if [ $? -eq 1 ]; then pass "exists exits 1 for missing local file"; else fail "exists missing exit code"; fi

OUT=$($BINARY exists --json "$TEST_DIR/a/same.txt" 2>/dev/null)
echo "$OUT" | grep -q '"exists": true'
if [ $? -eq 0 ]; then pass "exists --json emits JSON"; else fail "exists --json"; fi

OUT=$($BINARY hash "$TEST_DIR/a/same.txt" --algorithm SHA256 2>/dev/null)
echo "$OUT" | grep -q '^SHA256'
if [ $? -eq 0 ]; then pass "hash prints algorithm and digest"; else fail "hash output"; fi

OUT=$($BINARY hash --json "$TEST_DIR/a/same.txt" --algorithm MD5 2>/dev/null)
echo "$OUT" | grep -q '"algorithm": "MD5"'
if [ $? -eq 0 ]; then pass "hash --json emits JSON"; else fail "hash --json"; fi

$BINARY cmp "$TEST_DIR/a/same.txt" "$TEST_DIR/b/same.txt" >/dev/null 2>&1
if [ $? -eq 0 ]; then pass "cmp succeeds for identical files"; else fail "cmp identical"; fi

OUT=$($BINARY cmp --json "$TEST_DIR/a/same.txt" "$TEST_DIR/b/diff.txt" 2>/dev/null)
STATUS=$?
echo "$OUT" | grep -q '"identical": false'
if [ $STATUS -eq 1 ] && [ $? -eq 0 ]; then pass "cmp --json reports mismatches"; else fail "cmp --json"; fi

OUT=$($BINARY stat --json "$TEST_DIR/a/same.txt" 2>/dev/null)
echo "$OUT" | grep -q '"type": "file"'
if [ $? -eq 0 ]; then pass "stat --json emits file metadata"; else fail "stat --json"; fi

OUT=$($BINARY diff --json "$TEST_DIR/a" "$TEST_DIR/b" 2>/dev/null)
echo "$OUT" | grep -q '"identical": false'
if [ $? -eq 0 ]; then pass "diff --json emits structured differences"; else fail "diff --json"; fi

rm -rf "$TEST_DIR"

echo "Passed: $PASS"
echo "Failed: $FAIL"
[ "$FAIL" -eq 0 ]

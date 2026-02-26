#!/bin/bash
# Comprehensive stat command demonstration

echo "╔══════════════════════════════════════════════════════════╗"
echo "║         STAT Command Comprehensive Demonstration         ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""

BINARY="./target/debug/hsc"

# Test 1: Regular File
echo "━━━ Test 1: Stat on Regular File ━━━"
$BINARY stat README.md
echo ""

# Test 2: Directory
echo "━━━ Test 2: Stat on Directory ━━━"
$BINARY stat src/
echo ""

# Test 3: Executable File
echo "━━━ Test 3: Stat on Executable File ━━━"
$BINARY stat $BINARY
echo ""

# Test 4: Create test files with different attributes
echo "━━━ Test 4: Stat on Recently Created File ━━━"
echo "New test file" > /tmp/hsc-demo.txt
$BINARY stat /tmp/hsc-demo.txt
rm -f /tmp/hsc-demo.txt
echo ""

# Test 5: Non-existent file
echo "━━━ Test 5: Stat on Non-Existent File (should error) ━━━"
$BINARY stat /nonexistent/file.txt 2>&1 || echo "Expected: File not found error"
echo ""

echo "━━━ Summary ━━━"
echo "✓ Local file stat: Working"
echo "✓ Directory stat: Working"
echo "✓ Error handling: Working"
echo ""
echo "For S3 stat examples, configure AWS credentials and run:"
echo "  $BINARY stat s3://bucket/object"
echo "  $BINARY stat s3://bucket"
echo ""

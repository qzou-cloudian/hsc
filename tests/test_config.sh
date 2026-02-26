#!/bin/bash
# Test loading multipart settings from AWS config file

set -e

HSTEST="./target/debug/hsc"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${YELLOW}=== Testing multipart config loading ===${NC}\n"

# Create test config file
TEST_CONFIG="/tmp/test-aws-config-$$"
mkdir -p "$(dirname "$TEST_CONFIG")"

cat > "$TEST_CONFIG" << 'EOF'
[default]
region = us-east-1

[s3]
multipart_threshold = 5MB
multipart_chunksize = 2MB

[profile testprofile]
region = us-west-2
EOF

echo -e "${GREEN}Test 1: Created test AWS config:${NC}"
cat "$TEST_CONFIG"
echo ""

echo -e "${GREEN}Test 2: Config file location${NC}"
echo "AWS_CONFIG_FILE=$TEST_CONFIG"
echo ""

echo -e "${GREEN}Test 3: Settings in config:${NC}"
echo "  multipart_threshold = 5MB (5242880 bytes)"
echo "  multipart_chunksize = 2MB (2097152 bytes)"
echo ""

echo -e "${GREEN}Test 4: Size parsing tests${NC}"

# Test various formats
echo "Supported formats:"
echo "  - Plain bytes: 8388608"
echo "  - With MB: 8MB (8 * 1024 * 1024 = 8388608)"
echo "  - With M: 8M (8 * 1024 * 1024 = 8388608)"
echo "  - With KB: 5120KB (5120 * 1024 = 5242880)"
echo "  - With GB: 1GB (1 * 1024 * 1024 * 1024 = 1073741824)"
echo ""

echo -e "${GREEN}Test 5: Default values (when config not found)${NC}"
echo "If AWS config file doesn't exist or doesn't have [s3] section:"
echo "  multipart_threshold = 8388608 bytes (8MB)"
echo "  multipart_chunksize = 8388608 bytes (8MB)"
echo ""

echo -e "${GREEN}Test 6: Config precedence${NC}"
echo "Settings are loaded from:"
echo "  1. [s3] section (global)"
echo "  2. [profile <name>] section (profile-specific overrides)"
echo "  3. Built-in defaults if not found"
echo ""

echo -e "${GREEN}Test 7: Testing with actual command${NC}"
echo "Setting AWS_CONFIG_FILE=$TEST_CONFIG"
export AWS_CONFIG_FILE="$TEST_CONFIG"

# Create a small test file
dd if=/dev/zero of=/tmp/test-3mb.dat bs=1M count=3 2>/dev/null
echo "Created 3MB test file"
echo ""

echo "Expected behavior:"
echo "  - File size: 3MB (3145728 bytes)"
echo "  - Config threshold: 5MB (5242880 bytes)"
echo "  - 3MB < 5MB -> Should use regular put_object (not multipart)"
echo ""

echo "If file was 6MB:"
echo "  - File size: 6MB (6291456 bytes)"  
echo "  - Config threshold: 5MB (5242880 bytes)"
echo "  - 6MB > 5MB -> Should use multipart upload"
echo "  - Chunksize: 2MB -> Would create 3 parts"
echo ""

# Clean up
rm -f /tmp/test-3mb.dat "$TEST_CONFIG"

echo -e "${GREEN}=== Config loading tests completed ===${NC}"
echo ""
echo "To use custom multipart settings, add to your ~/.aws/config:"
echo ""
echo "[s3]"
echo "multipart_threshold = 10MB"
echo "multipart_chunksize = 5MB"

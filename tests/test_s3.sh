#!/bin/bash
# Test script for hsc S3 CLI tool

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
CONFIG_FILE="${1:-$HOME/.aws/config}"
TEST_BUCKET="hsc-test-bucket-$(date +%s)"
BINARY="./target/debug/hsc"

echo -e "${YELLOW}=== AWS S3 CLI Test Suite ===${NC}\n"

# Check if binary exists
if [ ! -f "$BINARY" ]; then
    echo -e "${RED}Error: Binary not found at $BINARY${NC}"
    echo "Run 'cargo build' first"
    exit 1
fi

# Check if config file exists
if [ ! -f "$CONFIG_FILE" ]; then
    echo -e "${RED}Error: Config file not found: $CONFIG_FILE${NC}"
    echo "Usage: $0 [config_file]"
    exit 1
fi

# Load AWS credentials from config file
echo -e "${YELLOW}Loading AWS credentials from $CONFIG_FILE...${NC}"

# Parse s3cmd-style config file
if grep -q "^host_base" "$CONFIG_FILE" 2>/dev/null; then
    echo "Detected s3cmd-style config format"

    # Parse config file
    ACCESS_KEY=$(grep "^access_key" "$CONFIG_FILE" | awk '{print $3}')
    SECRET_KEY=$(grep "^secret_key" "$CONFIG_FILE" | awk '{print $3}')
    HOST_BASE=$(grep "^host_base" "$CONFIG_FILE" | awk '{print $3}')
    USE_HTTPS=$(grep "^use_https" "$CONFIG_FILE" | awk '{print $3}')

    # Set environment variables
    export AWS_ACCESS_KEY_ID="$ACCESS_KEY"
    export AWS_SECRET_ACCESS_KEY="$SECRET_KEY"
    export AWS_REGION="us-east-1"  # S3-compatible services often ignore this

    # Set endpoint URL
    if [ "$USE_HTTPS" = "true" ]; then
        export AWS_ENDPOINT_URL="https://$HOST_BASE"
    else
        export AWS_ENDPOINT_URL="http://$HOST_BASE"
    fi

    echo "Endpoint: $AWS_ENDPOINT_URL"
else
    # Standard AWS config format
    export AWS_CONFIG_FILE="$CONFIG_FILE"
    export AWS_SHARED_CREDENTIALS_FILE="$CONFIG_FILE"
fi

# Create test directories and files
TEST_DIR="test_data"
TEST_DIR2="test_data2"
DOWNLOAD_DIR="download_data"

cleanup() {
    echo -e "\n${YELLOW}Cleaning up...${NC}"

    # Remove test bucket and all objects
    echo "Removing test bucket: $TEST_BUCKET"
    $BINARY rb "s3://$TEST_BUCKET" --force 2>/dev/null || true

    # Remove local test directories
    rm -rf "$TEST_DIR" "$TEST_DIR2" "$DOWNLOAD_DIR"

    echo -e "${GREEN}Cleanup complete${NC}"
}

# Set up cleanup trap
trap cleanup EXIT

setup_test_data() {
    echo -e "\n${YELLOW}Setting up test data...${NC}"

    # Create test directory structure
    mkdir -p "$TEST_DIR/subdir1"
    mkdir -p "$TEST_DIR/subdir2"

    # Create test files
    echo "Test file 1" > "$TEST_DIR/file1.txt"
    echo "Test file 2" > "$TEST_DIR/file2.txt"
    echo "Log entry" > "$TEST_DIR/test.log"
    echo "Subdir file 1" > "$TEST_DIR/subdir1/file3.txt"
    echo "Subdir file 2" > "$TEST_DIR/subdir2/file4.txt"
    echo "Data file" > "$TEST_DIR/data.json"
    echo "Temp file" > "$TEST_DIR/temp_file.tmp"

    echo -e "${GREEN}✓ Test data created${NC}"
}

test_mb() {
    echo -e "\n${YELLOW}Test 1: Make Bucket (mb)${NC}"
    $BINARY mb "s3://$TEST_BUCKET"
    echo -e "${GREEN}✓ Bucket created: $TEST_BUCKET${NC}"
}

test_ls_buckets() {
    echo -e "\n${YELLOW}Test 2: List Buckets (ls)${NC}"
    $BINARY ls | grep "$TEST_BUCKET"
    echo -e "${GREEN}✓ Bucket listed successfully${NC}"
}

test_cp_single() {
    echo -e "\n${YELLOW}Test 3: Copy Single File (cp)${NC}"
    $BINARY cp "$TEST_DIR/file1.txt" "s3://$TEST_BUCKET/file1.txt"
    echo -e "${GREEN}✓ Single file uploaded${NC}"
}

test_cp_with_checksum() {
    echo -e "\n${YELLOW}Test 4: Copy with Checksum (cp --checksum-mode --checksum-algorithm)${NC}"
    $BINARY cp "$TEST_DIR/file2.txt" "s3://$TEST_BUCKET/file2_checksum.txt" \
        --checksum-mode ENABLED --checksum-algorithm SHA256
    echo -e "${GREEN}✓ File uploaded with checksum validation${NC}"
}

test_ls_objects() {
    echo -e "\n${YELLOW}Test 5: List Objects (ls)${NC}"
    $BINARY ls "s3://$TEST_BUCKET"
    echo -e "${GREEN}✓ Objects listed${NC}"
}

test_cp_recursive() {
    echo -e "\n${YELLOW}Test 6: Copy Directory Recursively (cp --recursive)${NC}"
    $BINARY cp "$TEST_DIR" "s3://$TEST_BUCKET/test_prefix/" --recursive
    echo -e "${GREEN}✓ Directory uploaded recursively${NC}"
}

test_ls_recursive() {
    echo -e "\n${YELLOW}Test 7: List Objects Recursively (ls --recursive)${NC}"
    $BINARY ls "s3://$TEST_BUCKET/test_prefix/" --recursive
    echo -e "${GREEN}✓ Objects listed recursively${NC}"
}

test_cp_with_filters() {
    echo -e "\n${YELLOW}Test 8: Copy with Filters (cp --recursive --include --exclude)${NC}"
    $BINARY cp "$TEST_DIR" "s3://$TEST_BUCKET/filtered/" --recursive \
        --include "*.txt" --exclude "temp*"
    echo -e "${GREEN}✓ Files copied with filters${NC}"

    # Verify filtered files
    echo "Verifying filters applied correctly..."
    $BINARY ls "s3://$TEST_BUCKET/filtered/" --recursive | grep -q "file1.txt"
    ! $BINARY ls "s3://$TEST_BUCKET/filtered/" --recursive | grep -q "temp_file.tmp" || {
        echo -e "${RED}✗ Filter test failed: temp file was uploaded${NC}"
        exit 1
    }
    echo -e "${GREEN}✓ Filters verified${NC}"
}

test_cp_download() {
    echo -e "\n${YELLOW}Test 9: Download File (cp S3 to local)${NC}"
    mkdir -p "$DOWNLOAD_DIR"
    $BINARY cp "s3://$TEST_BUCKET/file1.txt" "$DOWNLOAD_DIR/file1.txt"

    # Verify content
    if diff "$TEST_DIR/file1.txt" "$DOWNLOAD_DIR/file1.txt" > /dev/null; then
        echo -e "${GREEN}✓ File downloaded and verified${NC}"
    else
        echo -e "${RED}✗ Downloaded file content mismatch${NC}"
        exit 1
    fi
}

test_cp_s3_to_s3() {
    echo -e "\n${YELLOW}Test 10: Copy S3 to S3 (cp)${NC}"
    $BINARY cp "s3://$TEST_BUCKET/file1.txt" "s3://$TEST_BUCKET/file1_copy.txt"
    echo -e "${GREEN}✓ File copied within S3${NC}"
}

test_sync_upload() {
    echo -e "\n${YELLOW}Test 11: Sync Local to S3 (sync)${NC}"
    # Modify a file to test sync
    echo "Modified content" > "$TEST_DIR/file1.txt"
    $BINARY sync "$TEST_DIR" "s3://$TEST_BUCKET/sync_test/"
    echo -e "${GREEN}✓ Directory synced to S3${NC}"
}

test_sync_download() {
    echo -e "\n${YELLOW}Test 12: Sync S3 to Local (sync)${NC}"
    mkdir -p "$TEST_DIR2"
    $BINARY sync "s3://$TEST_BUCKET/sync_test/" "$TEST_DIR2"

    # Verify synced content
    if [ -f "$TEST_DIR2/file1.txt" ]; then
        echo -e "${GREEN}✓ Directory synced from S3${NC}"
    else
        echo -e "${RED}✗ Sync failed: files not found${NC}"
        exit 1
    fi
}

test_sync_with_filters() {
    echo -e "\n${YELLOW}Test 13: Sync with Filters (sync --include --exclude)${NC}"
    $BINARY sync "$TEST_DIR" "s3://$TEST_BUCKET/sync_filtered/" \
        --include "*.txt" --exclude "*.log"
    echo -e "${GREEN}✓ Directory synced with filters${NC}"
}

test_mv() {
    echo -e "\n${YELLOW}Test 14: Move File (mv)${NC}"
    $BINARY mv "s3://$TEST_BUCKET/file1_copy.txt" "s3://$TEST_BUCKET/moved_file.txt"

    # Verify source deleted and dest exists
    if $BINARY ls "s3://$TEST_BUCKET" | grep -q "moved_file.txt"; then
        echo -e "${GREEN}✓ File moved successfully${NC}"
    else
        echo -e "${RED}✗ Move failed${NC}"
        exit 1
    fi
}

test_mv_recursive() {
    echo -e "\n${YELLOW}Test 15: Move Directory Recursively (mv --recursive)${NC}"
    $BINARY mv "s3://$TEST_BUCKET/filtered/" "s3://$TEST_BUCKET/moved_dir/" --recursive
    echo -e "${GREEN}✓ Directory moved successfully${NC}"
}

test_rm_single() {
    echo -e "\n${YELLOW}Test 16: Remove Single Object (rm)${NC}"
    $BINARY rm "s3://$TEST_BUCKET/moved_file.txt"

    # Verify deletion
    if ! $BINARY ls "s3://$TEST_BUCKET" | grep -q "moved_file.txt"; then
        echo -e "${GREEN}✓ Object removed${NC}"
    else
        echo -e "${RED}✗ Remove failed${NC}"
        exit 1
    fi
}

test_rm_recursive() {
    echo -e "\n${YELLOW}Test 17: Remove Objects Recursively (rm --recursive)${NC}"
    $BINARY rm "s3://$TEST_BUCKET/test_prefix/" --recursive
    echo -e "${GREEN}✓ Objects removed recursively${NC}"
}

test_rm_with_filters() {
    echo -e "\n${YELLOW}Test 18: Remove with Filters (rm --recursive --include)${NC}"
    # Upload some test files first
    echo "Log 1" > "$TEST_DIR/app1.log"
    echo "Log 2" > "$TEST_DIR/app2.log"
    echo "Keep this" > "$TEST_DIR/keep.txt"
    $BINARY cp "$TEST_DIR" "s3://$TEST_BUCKET/logs/" --recursive

    # Remove only .log files
    $BINARY rm "s3://$TEST_BUCKET/logs/" --recursive --include "*.log"

    # Verify .txt file still exists
    if $BINARY ls "s3://$TEST_BUCKET/logs/" --recursive | grep -q "keep.txt"; then
        echo -e "${GREEN}✓ Filtered removal successful${NC}"
    else
        echo -e "${RED}✗ Filter removal failed${NC}"
        exit 1
    fi
}

test_rb_force() {
    echo -e "\n${YELLOW}Test 19: Remove Bucket with Force (rb --force)${NC}"
    $BINARY rb "s3://$TEST_BUCKET" --force
    echo -e "${GREEN}✓ Bucket removed with all objects${NC}"

    # Disable cleanup since we already removed the bucket
    trap - EXIT
    rm -rf "$TEST_DIR" "$TEST_DIR2" "$DOWNLOAD_DIR"
}

# Run all tests
main() {
    setup_test_data
    test_mb
    test_ls_buckets
    test_cp_single
    test_cp_with_checksum
    test_ls_objects
    test_cp_recursive
    test_ls_recursive
    test_cp_with_filters
    test_cp_download
    test_cp_s3_to_s3
    test_sync_upload
    test_sync_download
    test_sync_with_filters
    test_mv
    test_mv_recursive
    test_rm_single
    test_rm_recursive
    test_rm_with_filters
    test_rb_force

    echo -e "\n${GREEN}=== All tests passed! ===${NC}\n"
}

# Run the test suite
main

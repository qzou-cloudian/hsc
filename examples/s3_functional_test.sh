#!/bin/bash

# S3 Functional Test Script
# Tests bucket operations, object put/get with various sizes, and range requests

# Don't exit on error - we want to count all failures
set +e

BINARY="./target/debug/hsc"

# Configuration from config file
BUCKET_NAME="test-bucket-$(date +%s)"
TEST_DIR="./test_data"

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Counters
SUCCESS_COUNT=0
ERROR_COUNT=0

# Object sizes to test
SIZES=("1k" "8k" "64k" "512k" "1m" "8m" "16m" "24m" "32m" "64m")

echo "========================================="
echo "S3 Functional Test"
echo "Endpoint: $ENDPOINT"
echo "Bucket: $BUCKET_NAME"
echo "========================================="

# Create test data directory
mkdir -p "$TEST_DIR"

# Function to print success message
success() {
    echo -e "${GREEN}✓ $1${NC}"
    ((SUCCESS_COUNT++))
}

# Function to print error message
error() {
    echo -e "${RED}✗ $1${NC}"
    ((ERROR_COUNT++))
}

# Function to print info message
info() {
    echo -e "${YELLOW}→ $1${NC}"
}

# Function to create test file
create_test_file() {
    local size=$1
    local filename="$TEST_DIR/testfile_${size}.dat"

    info "Creating test file: $filename (size: $size)"

    # Convert size to bytes for dd
    case $size in
        *k) dd if=/dev/urandom of="$filename" bs=1024 count=${size%k} status=none ;;
        *m) dd if=/dev/urandom of="$filename" bs=1048576 count=${size%m} status=none ;;
    esac

    success "Created $filename ($(du -h "$filename" | cut -f1))"
}

# Step 1: Create bucket
echo ""
info "Step 1: Creating bucket '$BUCKET_NAME'..."
if $BINARY mb "s3://$BUCKET_NAME"; then
    success "Bucket created successfully"
else
    error "Failed to create bucket"
    exit 1
fi

# Step 2: Create test files and upload objects
echo ""
info "Step 2: Creating test files and uploading objects..."
for size in "${SIZES[@]}"; do
    create_test_file "$size"

    filename="$TEST_DIR/testfile_${size}.dat"
    object_key="testfile_${size}.dat"

    info "Uploading $object_key..."
    if $BINARY cp "$filename" "s3://$BUCKET_NAME/$object_key"; then
        success "Uploaded $object_key"
    else
        error "Failed to upload $object_key"
    fi
done

# List objects to verify
echo ""
info "Listing objects in bucket..."
$BINARY ls "s3://$BUCKET_NAME"

# Step 2b: Test Multipart Upload (via $BINARY cp for large files)
echo ""
info "Step 2b: Testing Multipart Upload (via $BINARY cp)..."
MULTIPART_SIZES=("1m" "16m" "32m")
mkdir -p "$TEST_DIR/multipart"

for part_size in "${MULTIPART_SIZES[@]}"; do
    info "Creating multipart test file with ${part_size} parts..."

    multipart_file="$TEST_DIR/multipart/multipart_${part_size}_parts.dat"
    object_key="multipart_${part_size}_parts.dat"

    # Create 3 parts of the specified size and combine into one file
    part1="$TEST_DIR/multipart/part1_${part_size}.dat"
    part2="$TEST_DIR/multipart/part2_${part_size}.dat"
    part3="$TEST_DIR/multipart/part3_${part_size}.dat"

    case $part_size in
        1m)
            dd if=/dev/urandom of="$part1" bs=1048576 count=1 status=none
            dd if=/dev/urandom of="$part2" bs=1048576 count=1 status=none
            dd if=/dev/urandom of="$part3" bs=1048576 count=1 status=none
            ;;
        16m)
            dd if=/dev/urandom of="$part1" bs=1048576 count=16 status=none
            dd if=/dev/urandom of="$part2" bs=1048576 count=16 status=none
            dd if=/dev/urandom of="$part3" bs=1048576 count=16 status=none
            ;;
        32m)
            dd if=/dev/urandom of="$part1" bs=1048576 count=32 status=none
            dd if=/dev/urandom of="$part2" bs=1048576 count=32 status=none
            dd if=/dev/urandom of="$part3" bs=1048576 count=32 status=none
            ;;
    esac

    # Combine parts into one file
    cat "$part1" "$part2" "$part3" > "$multipart_file"
    original_md5=$(md5sum "$multipart_file" | cut -d' ' -f1)

    info "Uploading $object_key via $BINARY cp (multipart for large files)..."
    if $BINARY cp "$multipart_file" "s3://$BUCKET_NAME/$object_key"; then
        success "Uploaded $object_key"

        # Download and verify integrity
        download_file="$TEST_DIR/multipart/downloaded_${part_size}.dat"
        info "Downloading multipart object for verification..."
        if $BINARY cp "s3://$BUCKET_NAME/$object_key" "$download_file" > /dev/null 2>&1; then
            download_md5=$(md5sum "$download_file" | cut -d' ' -f1)

            if [ "$original_md5" = "$download_md5" ]; then
                success "Multipart upload integrity verified for $object_key (MD5: $download_md5)"
            else
                error "Multipart upload integrity check failed for $object_key"
                error "  Expected MD5: $original_md5"
                error "  Download MD5: $download_md5"
            fi
        else
            error "Failed to download multipart object $object_key for verification"
        fi
    else
        error "Failed to upload $object_key"
    fi

    # Clean up part files
    rm -f "$part1" "$part2" "$part3"
done

echo ""
info "Listing all objects (including multipart uploads)..."
$BINARY ls "s3://$BUCKET_NAME"

# Step 3: Download objects (full size) with integrity verification
echo ""
info "Step 3: Downloading objects (full size) and verifying data integrity..."
mkdir -p "$TEST_DIR/downloads"
for size in "${SIZES[@]}"; do
    object_key="testfile_${size}.dat"
    download_file="$TEST_DIR/downloads/testfile_${size}.dat"
    original_file="$TEST_DIR/testfile_${size}.dat"

    info "Downloading $object_key..."
    if $BINARY cp "s3://$BUCKET_NAME/$object_key" "$download_file"; then

        # Verify file size matches
        original_size=$(stat -f%z "$original_file" 2>/dev/null || stat -c%s "$original_file")
        download_size=$(stat -f%z "$download_file" 2>/dev/null || stat -c%s "$download_file")

        if [ "$original_size" -ne "$download_size" ]; then
            error "Size mismatch for $object_key (expected: $original_size, got: $download_size)"
            continue
        fi

        # Verify content integrity using MD5 checksum
        original_md5=$(md5sum "$original_file" | cut -d' ' -f1)
        download_md5=$(md5sum "$download_file" | cut -d' ' -f1)

        if [ "$original_md5" = "$download_md5" ]; then
            success "Downloaded and verified $object_key (size: $download_size bytes, MD5: $download_md5)"
        else
            error "Data integrity check failed for $object_key (MD5 mismatch)"
            error "  Original MD5: $original_md5"
            error "  Download MD5: $download_md5"
            continue
        fi

        # Verify object metadata via $BINARY stat
        stat_output=$($BINARY stat "s3://$BUCKET_NAME/$object_key" 2>/dev/null)
        response_etag=$(echo "$stat_output" | grep "^ETag" | sed 's/ETag *: //' | tr -d '"')
        response_content_length=$(echo "$stat_output" | grep "^Size" | sed 's/Size *: //; s/ bytes.*//')

        # Check ETag header
        if [ -n "$response_etag" ]; then
            if [ "$response_etag" = "$original_md5" ]; then
                success "Response ETag matches MD5: $response_etag"
            elif [[ "$response_etag" == *"-"* ]]; then
                # Multipart ETag format (MD5-partcount), just verify it exists
                success "Response ETag (multipart): $response_etag"
            else
                error "Response ETag mismatch (expected: $original_md5, got: $response_etag)"
            fi
        else
            error "Response ETag not found for $object_key"
        fi

        # Check Content-Length header
        if [ -n "$response_content_length" ]; then
            if [ "$response_content_length" -eq "$original_size" ]; then
                success "Response Content-Length correct: $response_content_length"
            else
                error "Response Content-Length mismatch (expected: $original_size, got: $response_content_length)"
            fi
        else
            error "Response Content-Length not found for $object_key"
        fi
    else
        error "Failed to download $object_key"
    fi
done

# Step 4: Test range requests with integrity verification
echo ""
info "Step 4: Testing range requests and verifying data integrity..."
mkdir -p "$TEST_DIR/ranges"

# Function to verify range data integrity
verify_range() {
    local original_file=$1
    local range_spec=$2
    local range_file=$3

    # Parse range specification (bytes=start-end)
    local range_nums=$(echo "$range_spec" | sed 's/bytes=//')
    local start=$(echo "$range_nums" | cut -d'-' -f1)
    local end=$(echo "$range_nums" | cut -d'-' -f2)
    local expected_size=$((end - start + 1))

    # Extract expected bytes from original file
    local expected_file="${range_file}.expected"
    dd if="$original_file" of="$expected_file" bs=1 skip=$start count=$expected_size 2>/dev/null

    # Compare MD5 checksums
    local expected_md5=$(md5sum "$expected_file" | cut -d' ' -f1)
    local range_md5=$(md5sum "$range_file" | cut -d' ' -f1)

    rm -f "$expected_file"

    if [ "$expected_md5" = "$range_md5" ]; then
        return 0
    else
        return 1
    fi
}

# Test different ranges on 1m file
test_ranges=("bytes=0-1023" "bytes=1024-2047" "bytes=0-511" "bytes=512000-1048575")
for range in "${test_ranges[@]}"; do
    object_key="testfile_1m.dat"
    original_file="$TEST_DIR/testfile_1m.dat"
    range_file="$TEST_DIR/ranges/range_$(echo $range | tr '=' '_' | tr '-' '_').dat"

    info "Downloading $object_key with range: $range..."
    if $BINARY cat --range "$range" "s3://$BUCKET_NAME/$object_key" > "$range_file" 2>/dev/null; then

        file_size=$(stat -f%z "$range_file" 2>/dev/null || stat -c%s "$range_file")

        # Verify data integrity
        if verify_range "$original_file" "$range" "$range_file"; then
            success "Range request verified: $range (size: $file_size bytes, integrity: OK)"
        else
            error "Range request data integrity failed: $range"
        fi
    else
        error "Range request failed: $range"
    fi
done

# Test range on 64m file
info "Testing range on large file (64m)..."
range="bytes=0-1048575"
original_file="$TEST_DIR/testfile_64m.dat"
range_file="$TEST_DIR/ranges/range_64m_first_1m.dat"
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/testfile_64m.dat" > "$range_file" 2>/dev/null; then

    file_size=$(stat -f%z "$range_file" 2>/dev/null || stat -c%s "$range_file")

    # Verify data integrity
    if verify_range "$original_file" "$range" "$range_file"; then
        success "Range request on 64m file verified (size: $file_size bytes, integrity: OK)"
    else
        error "Range request on 64m file data integrity failed"
    fi

    # Verify range response size
    expected_range_size=1048576
    if [ "$file_size" -eq "$expected_range_size" ]; then
        success "Range response size correct: $file_size"
    else
        error "Range response size mismatch (expected: $expected_range_size, got: $file_size)"
    fi
else
    error "Range request on 64m file failed"
fi

# Additional range tests on different file sizes
info "Testing additional range patterns..."
# Test middle range on 8m file
range="bytes=4194304-5242879"
original_file="$TEST_DIR/testfile_8m.dat"
range_file="$TEST_DIR/ranges/range_8m_middle.dat"
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/testfile_8m.dat" > "$range_file" 2>/dev/null; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Middle range on 8m file verified: $range"
    else
        error "Middle range on 8m file data integrity failed"
    fi
fi

# Test last bytes of 32m file
range="bytes=33553408-33554431"
original_file="$TEST_DIR/testfile_32m.dat"
range_file="$TEST_DIR/ranges/range_32m_last.dat"
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/testfile_32m.dat" > "$range_file" 2>/dev/null; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Last 1KB of 32m file verified: $range"
    else
        error "Last 1KB of 32m file data integrity failed"
    fi
fi

# Test range requests on multipart uploaded objects
echo ""
info "Testing range requests on multipart uploaded objects..."

# Test on 1m parts object (3MB total)
info "Testing ranges on multipart object with 1m parts (3MB total)..."
original_file="$TEST_DIR/multipart/multipart_1m_parts.dat"

# Range within first part
range="bytes=0-524287"
range_file="$TEST_DIR/ranges/mp_1m_first_half_part1.dat"
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/multipart_1m_parts.dat" > "$range_file" 2>/dev/null; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Multipart 1m: First half of part 1 verified"
    else
        error "Multipart 1m: First half of part 1 integrity failed"
    fi
fi

# Range spanning part 1 and part 2 boundary
range="bytes=1048000-1049599"
range_file="$TEST_DIR/ranges/mp_1m_boundary.dat"
info "Testing CRITICAL: Range across part boundary (part 1->2)..."
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/multipart_1m_parts.dat" > "$range_file" 2>/tmp/range_error.log; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Multipart 1m: Range across part boundary (part 1->2) verified"
    else
        error "Multipart 1m: Range across part boundary integrity failed"
    fi
else
    error "⚠️  FAILED: Multipart 1m range across part 1->2 boundary ($range)"
    error "    This range spans the 1MB boundary between parts"
    cat /tmp/range_error.log | grep -i "error" || echo "    Check /tmp/range_error.log for details"
fi

# Range in middle part
range="bytes=1572864-2097151"
range_file="$TEST_DIR/ranges/mp_1m_middle_part.dat"
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/multipart_1m_parts.dat" > "$range_file" 2>/dev/null; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Multipart 1m: Middle of part 2 verified"
    else
        error "Multipart 1m: Middle of part 2 integrity failed"
    fi
fi

# Test on 16m parts object (48MB total)
info "Testing ranges on multipart object with 16m parts (48MB total)..."
original_file="$TEST_DIR/multipart/multipart_16m_parts.dat"

# Large range within first part
range="bytes=0-8388607"
range_file="$TEST_DIR/ranges/mp_16m_first_8mb.dat"
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/multipart_16m_parts.dat" > "$range_file" 2>/dev/null; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Multipart 16m: First 8MB of part 1 verified"
    else
        error "Multipart 16m: First 8MB integrity failed"
    fi
fi

# Range spanning part boundary (part 1 -> part 2)
range="bytes=16776192-16778239"
range_file="$TEST_DIR/ranges/mp_16m_boundary.dat"
info "Testing CRITICAL: Range across 16MB part boundary..."
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/multipart_16m_parts.dat" > "$range_file" 2>/tmp/range_error.log; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Multipart 16m: Range across part boundary (16MB boundary) verified"
    else
        error "Multipart 16m: Range across part boundary integrity failed"
    fi
else
    error "⚠️  FAILED: Multipart 16m range across part 1->2 boundary ($range)"
    error "    This range spans the 16MB boundary between parts"
    cat /tmp/range_error.log | grep -i "error" || echo "    Check /tmp/range_error.log for details"
fi

# Range in third part
range="bytes=40000000-41000000"
range_file="$TEST_DIR/ranges/mp_16m_third_part.dat"
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/multipart_16m_parts.dat" > "$range_file" 2>/dev/null; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Multipart 16m: Range in part 3 verified"
    else
        error "Multipart 16m: Range in part 3 integrity failed"
    fi
fi

# Test on 32m parts object (96MB total)
info "Testing ranges on multipart object with 32m parts (96MB total)..."
original_file="$TEST_DIR/multipart/multipart_32m_parts.dat"

# Range at end of first part
range="bytes=33554000-33554431"
range_file="$TEST_DIR/ranges/mp_32m_end_part1.dat"
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/multipart_32m_parts.dat" > "$range_file" 2>/dev/null; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Multipart 32m: End of part 1 verified"
    else
        error "Multipart 32m: End of part 1 integrity failed"
    fi
fi

# Range spanning part 2 and part 3 boundary (at 64MB mark)
range="bytes=67108000-67109000"
range_file="$TEST_DIR/ranges/mp_32m_boundary_2_3.dat"
info "Testing CRITICAL: Range across 32MB part boundary (part 2->3)..."
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/multipart_32m_parts.dat" > "$range_file" 2>/tmp/range_error.log; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Multipart 32m: Range across part 2->3 boundary (64MB) verified"
    else
        error "Multipart 32m: Range across part 2->3 boundary integrity failed"
    fi
else
    error "⚠️  FAILED: Multipart 32m range across part 2->3 boundary ($range)"
    error "    This range spans the 64MB boundary between parts 2 and 3"
    cat /tmp/range_error.log | grep -i "error" || echo "    Check /tmp/range_error.log for details"
fi

# Large range spanning all parts
range="bytes=10000000-90000000"
range_file="$TEST_DIR/ranges/mp_32m_span_all.dat"
info "Testing CRITICAL: Large range spanning all 3 parts..."
if $BINARY cat --range "$range" "s3://$BUCKET_NAME/multipart_32m_parts.dat" > "$range_file" 2>/tmp/range_error.log; then

    if verify_range "$original_file" "$range" "$range_file"; then
        success "Multipart 32m: Large range spanning all parts verified (80MB)"
    else
        error "Multipart 32m: Large range spanning all parts integrity failed"
    fi
else
    error "⚠️  FAILED: Multipart 32m large range spanning all parts ($range)"
    error "    This range spans from part 1 through part 3 (80MB of data)"
    cat /tmp/range_error.log | grep -i "error" || echo "    Check /tmp/range_error.log for details"
fi

# Step 5: Delete all objects
echo ""
info "Step 5: Deleting all objects..."
for size in "${SIZES[@]}"; do
    object_key="testfile_${size}.dat"

    info "Deleting $object_key..."
    if $BINARY rm "s3://$BUCKET_NAME/$object_key"; then
        success "Deleted $object_key"
    else
        error "Failed to delete $object_key"
    fi
done

# Delete multipart uploaded objects
for part_size in "${MULTIPART_SIZES[@]}"; do
    object_key="multipart_${part_size}_parts.dat"

    info "Deleting $object_key..."
    if $BINARY rm "s3://$BUCKET_NAME/$object_key"; then
        success "Deleted $object_key"
    else
        error "Failed to delete $object_key"
    fi
done

# Verify bucket is empty
echo ""
info "Verifying bucket is empty..."
object_count=$($BINARY ls "s3://$BUCKET_NAME" | wc -l)
if [ "$object_count" -eq 0 ]; then
    success "Bucket is empty"
else
    error "Bucket still contains objects"
fi

# Step 6: Delete bucket
echo ""
info "Step 6: Deleting bucket '$BUCKET_NAME'..."
if $BINARY rb "s3://$BUCKET_NAME"; then
    success "Bucket deleted successfully"
else
    error "Failed to delete bucket"
fi

# Cleanup local test files
echo ""
info "Cleaning up local test files..."
rm -rf "$TEST_DIR"
success "Cleanup complete"

echo ""
echo "========================================="
echo "           TEST RESULTS SUMMARY         "
echo "========================================="
echo -e "${BLUE}Total Tests Run: $((SUCCESS_COUNT + ERROR_COUNT))${NC}"
echo -e "${GREEN}✓ Passed: $SUCCESS_COUNT${NC}"
echo -e "${RED}✗ Failed: $ERROR_COUNT${NC}"
echo "========================================="

if [ $ERROR_COUNT -eq 0 ]; then
    echo -e "${GREEN}🎉 All tests completed successfully!${NC}"
else
    echo -e "${YELLOW}⚠️  Some tests failed. Please review the output above.${NC}"
fi
echo "========================================="

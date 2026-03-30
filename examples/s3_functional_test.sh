#!/bin/bash

# S3 Functional Test Script
# Tests bucket operations, object put/get with various sizes, and range requests

# Don't exit on error - we want to count all failures
set +e

# When HSC_RDMA is set to a non-false value, build and use the rdma-featured binary.
if [[ -n "$HSC_RDMA" && "$HSC_RDMA" != "false" && "$HSC_RDMA" != "0" ]]; then
    if [[ "$HSC_RDMA" == "cuobj" || "$HSC_RDMA" == "auto" ]]; then
        echo "HSC_RDMA=$HSC_RDMA detected — building with --features cuobj ..."
        cargo build --features cuobj 2>&1 | tail -3
    else
        echo "HSC_RDMA=$HSC_RDMA detected — building with --features rdma ..."
        cargo build --features rdma 2>&1 | tail -3
    fi
    BINARY="./target/debug/hsc"
else
    BINARY="./target/debug/hsc"
fi

# SSE configuration — set HSC_SSE to enable server-side encryption for all tests.
#
#   AES256    — S3-managed AES-256 (transparent to reads; all tests work normally)
#   aws:kms   — AWS KMS encryption (set HSC_SSE_KMS_KEY_ID for a specific key)
#   sse-c     — Customer-provided AES-256 key (set HSC_SSE_C_KEY or auto-generated;
#               'hsc cmp --range' does not yet accept SSE-C keys so range-verification
#               steps are skipped in this mode)
#
# Examples:
#   HSC_SSE=AES256 ./examples/s3_functional_test.sh
#   HSC_SSE=aws:kms HSC_SSE_KMS_KEY_ID=arn:aws:kms:us-east-1:123:key/abc ./examples/s3_functional_test.sh
#   HSC_SSE=sse-c ./examples/s3_functional_test.sh             # auto-generates a key
#   HSC_SSE=sse-c HSC_SSE_C_KEY=<base64-32-bytes> ./examples/s3_functional_test.sh
HSC_SSE_KMS_KEY_ID="${HSC_SSE_KMS_KEY_ID:-}"
HSC_SSE_C_KEY="${HSC_SSE_C_KEY:-}"
SSE_UPLOAD_ARGS=""    # injected into every local→S3 cp command
SSE_DOWNLOAD_ARGS=""  # injected into every S3→local cp command (non-empty for sse-c only)
SSE_COPY_ARGS=""      # injected into every S3→S3 cp command

if [[ -n "$HSC_SSE" ]]; then
    case "${HSC_SSE,,}" in
        aes256)
            SSE_UPLOAD_ARGS="--sse AES256"
            SSE_COPY_ARGS="--sse AES256"
            ;;
        aws:kms)
            SSE_UPLOAD_ARGS="--sse aws:kms"
            SSE_COPY_ARGS="--sse aws:kms"
            if [[ -n "$HSC_SSE_KMS_KEY_ID" ]]; then
                SSE_UPLOAD_ARGS+=" --sse-kms-key-id $HSC_SSE_KMS_KEY_ID"
                SSE_COPY_ARGS+=" --sse-kms-key-id $HSC_SSE_KMS_KEY_ID"
            fi
            ;;
        sse-c)
            if [[ -z "$HSC_SSE_C_KEY" ]]; then
                HSC_SSE_C_KEY=$(openssl rand -base64 32)
                echo "HSC_SSE_C_KEY not set — auto-generated: $HSC_SSE_C_KEY"
            fi
            SSE_UPLOAD_ARGS="--sse-c AES256 --sse-c-key $HSC_SSE_C_KEY"
            SSE_DOWNLOAD_ARGS="--sse-c AES256 --sse-c-key $HSC_SSE_C_KEY"
            SSE_COPY_ARGS="--sse-c AES256 --sse-c-key $HSC_SSE_C_KEY --sse-c-copy-source AES256 --sse-c-copy-source-key $HSC_SSE_C_KEY"
            ;;
        *)
            echo "Warning: Unknown HSC_SSE='$HSC_SSE'. Valid values: AES256, aws:kms, sse-c"
            ;;
    esac
fi

# Bucket name: first positional argument, or auto-generated.
# When a bucket is supplied the create (Step 1) and delete (Step 10) steps are skipped,
# allowing the script to be run against a pre-existing bucket.
#
# Usage:
#   ./examples/s3_functional_test.sh                    # create+delete a temp bucket
#   ./examples/s3_functional_test.sh my-bucket          # use existing bucket, skip mb/rb
BUCKET_NAME="${1:-test-bucket-$(date +%s)}"
BUCKET_PROVIDED="${1:+true}"   # non-empty when caller supplied a bucket name
TEST_DIR="./test_data"
RESULTS_DIR=$(mktemp -d)
trap 'rm -rf "$RESULTS_DIR"' EXIT

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Counters
SUCCESS_COUNT=0
ERROR_COUNT=0
FAILED_TESTS=()   # accumulates every failure message for the end-of-run summary
FAILED_CMDS=()    # parallel array: rerun command for each failure

# Object sizes to test
SIZES=("1k" "8k" "64k" "512k" "1m" "8m" "16m" "24m" "32m" "64m")

echo "========================================="
echo "S3 Functional Test"
echo "Endpoint: $ENDPOINT"
echo "Bucket: $BUCKET_NAME"
echo "SSE Mode: ${HSC_SSE:-none}"
echo "========================================="

# Create test data directory
mkdir -p "$TEST_DIR"

# Function to print success message
success() {
    echo -e "${GREEN}✓ $1${NC}"
    ((SUCCESS_COUNT++))
}

# Function to print error message; optional $2 = rerun command string
error() {
    echo -e "${RED}✗ $1${NC}"
    ((ERROR_COUNT++))
    FAILED_TESTS+=("$1")
    FAILED_CMDS+=("${2:-}")
}

# Function to print info message
info() {
    echo -e "${YELLOW}→ $1${NC}"
}

# Timing helpers
SCRIPT_START=$SECONDS
STEP_START=$SECONDS

step_time() {
    local elapsed=$((SECONDS - STEP_START))
    local mins=$((elapsed / 60))
    local secs=$((elapsed % 60))
    if [ $mins -gt 0 ]; then
        printf "${YELLOW}  ⏱  %dm %02ds${NC}\n" $mins $secs
    else
        echo -e "${YELLOW}  ⏱  ${secs}s${NC}"
    fi
    STEP_START=$SECONDS
}

# Collect PASS:/FAIL:/INFO:/RERUN: lines written by parallel subshells to per-job files.
# Each FAIL: line may be immediately followed by a RERUN: line that carries the
# reproduce command; collect_results attaches it to the most recent failure entry.
collect_results() {
    for f in "$RESULTS_DIR"/job_*; do
        [ -f "$f" ] || continue
        while IFS= read -r line; do
            case "$line" in
                PASS:*) success "${line#PASS:}" ;;
                FAIL:*) error "${line#FAIL:}" ;;
                RERUN:*)
                    # Attach to the last recorded failure (parallel array index)
                    if [ ${#FAILED_TESTS[@]} -gt 0 ]; then
                        FAILED_CMDS[$(( ${#FAILED_TESTS[@]} - 1 ))]="${line#RERUN:}"
                    fi
                    ;;
                INFO:*) info "${line#INFO:}" ;;
                *) echo "$line" ;;
            esac
        done < "$f"
    done
    rm -f "$RESULTS_DIR"/job_*
}

# SSE-aware full-file comparison (local file vs S3 object).
_hsc_cmp() {
    local local_file="$1" s3uri="$2"
    # shellcheck disable=SC2086
    $BINARY cmp $SSE_DOWNLOAD_ARGS "$local_file" "$s3uri" 2>/dev/null
}

# Function to create test file
create_test_file() {
    local size=$1
    local filename="$TEST_DIR/testfile_${size}.dat"

    info "Creating test file: $filename (size: $size)"

    # Convert size to bytes for dd
    case $size in
        *k) dd if=/dev/random of="$filename" bs=1024 count=${size%k} status=none ;;
        *m) dd if=/dev/random of="$filename" bs=1048576 count=${size%m} status=none ;;
    esac

    success "Created $filename ($(du -h "$filename" | cut -f1))"
}

# Step 1: Create bucket
echo ""
info "Step 1: Creating bucket '$BUCKET_NAME'..."
if $BINARY mb --ignore-existing "s3://$BUCKET_NAME"; then
    success "Bucket ready: $BUCKET_NAME"
else
    error "Failed to create bucket $BUCKET_NAME"
    exit 1
fi

# Step 2: Create test files and upload objects
step_time
echo ""
info "Step 2: Creating test files and uploading objects..."
# Create all test files in parallel (setup)
for size in "${SIZES[@]}"; do
    (
        filename="$TEST_DIR/testfile_${size}.dat"
        case $size in
            *k) dd if=/dev/random of="$filename" bs=1024 count=${size%k} status=none ;;
            *m) dd if=/dev/random of="$filename" bs=1048576 count=${size%m} status=none ;;
        esac
    ) &
done
wait
info "All test files created"

# Upload all objects at once
info "Uploading test files to S3..."
# shellcheck disable=SC2086
if $BINARY sync $SSE_UPLOAD_ARGS "$TEST_DIR/" "s3://$BUCKET_NAME/" --exclude "*/multipart/*" --exclude "*/chunk_boundary/*" --exclude "*/chunk_downloads/*" --exclude "*/ec/*" --exclude "*/ec_dl/*" --exclude "*/sync_test/*" 2>/dev/null; then
    success "Uploaded ${#SIZES[@]} test files via sync"
else
    error "Failed to upload test files"
    exit 1
fi

# List objects to verify
echo ""
info "Listing objects in bucket..."
$BINARY ls "s3://$BUCKET_NAME"

# Step 2b: Test Multipart Upload (via $BINARY cp for large files)
step_time
echo ""
info "Step 2b: Testing Multipart Upload (via $BINARY cp)..."
MULTIPART_SIZES=("1m" "16m" "32m")
mkdir -p "$TEST_DIR/multipart"

_JOB=0
for part_size in "${MULTIPART_SIZES[@]}"; do
    (
        multipart_file="$TEST_DIR/multipart/multipart_${part_size}_parts.dat"
        object_key="multipart_${part_size}_parts.dat"
        part1="$TEST_DIR/multipart/part1_${part_size}.dat"
        part2="$TEST_DIR/multipart/part2_${part_size}.dat"
        part3="$TEST_DIR/multipart/part3_${part_size}.dat"

        echo "INFO:Creating multipart test file with ${part_size} parts..."
        case $part_size in
            1m)  count=1 ;;
            16m) count=16 ;;
            32m) count=32 ;;
        esac
        dd if=/dev/random of="$part1" bs=1048576 count=$count status=none &
        dd if=/dev/random of="$part2" bs=1048576 count=$count status=none &
        dd if=/dev/random of="$part3" bs=1048576 count=$count status=none &
        wait

        # Combine parts into one file
        cat "$part1" "$part2" "$part3" > "$multipart_file"

        echo "INFO:Uploading $object_key via $BINARY cp (multipart for large files)..."
        if $BINARY cp $SSE_UPLOAD_ARGS "$multipart_file" "s3://$BUCKET_NAME/$object_key" >/dev/null 2>&1; then
            echo "PASS:Uploaded $object_key"

            # Verify full-object integrity using hsc cmp
            echo "INFO:Verifying multipart upload integrity for $object_key..."
            if _hsc_cmp "$multipart_file" "s3://$BUCKET_NAME/$object_key"; then
                echo "PASS:Multipart upload integrity verified for $object_key"
            else
                echo "FAIL:Multipart upload integrity check failed for $object_key"
                _total_mp=$((count * 3 * 1048576))
                echo "RERUN:truncate -s $_total_mp /tmp/$object_key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$object_key s3://\$BUCKET_NAME/$object_key && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/$object_key /tmp/${object_key}_dl && cmp /tmp/$object_key /tmp/${object_key}_dl"
            fi
        else
            echo "FAIL:Failed to upload $object_key"
            _total_mp=$((count * 3 * 1048576))
            echo "RERUN:truncate -s $_total_mp /tmp/$object_key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$object_key s3://\$BUCKET_NAME/$object_key"
        fi

        # Clean up part files
        rm -f "$part1" "$part2" "$part3"
    ) > "$RESULTS_DIR/job_${_JOB}" &
    ((_JOB++))
done
wait
collect_results

echo ""
info "Listing all objects (including multipart uploads)..."
$BINARY ls "s3://$BUCKET_NAME"

# Step 3: Download objects (full size) with integrity verification
step_time
echo ""
info "Step 3: Downloading objects (full size) and verifying data integrity..."
mkdir -p "$TEST_DIR/downloads"
_JOB=0
for size in "${SIZES[@]}"; do
    (
        object_key="testfile_${size}.dat"
        download_file="$TEST_DIR/downloads/testfile_${size}.dat"
        original_file="$TEST_DIR/testfile_${size}.dat"
        case $size in
            *k) _sz=$((${size%k} * 1024)) ;;
            *m) _sz=$((${size%m} * 1048576)) ;;
        esac

        echo "INFO:Downloading $object_key..."
        if ! $BINARY cp $SSE_DOWNLOAD_ARGS "s3://$BUCKET_NAME/$object_key" "$download_file" >/dev/null 2>&1; then
            echo "FAIL:Failed to download $object_key"
            echo "RERUN:truncate -s $_sz /tmp/$object_key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$object_key s3://\$BUCKET_NAME/$object_key && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/$object_key /tmp/${object_key}_dl"
            exit 0
        fi

        original_size=$(stat -c%s "$original_file")
        download_size=$(stat -c%s "$download_file")

        if [ "$original_size" -ne "$download_size" ]; then
            echo "FAIL:Size mismatch for $object_key (expected: $original_size, got: $download_size)"
            echo "RERUN:truncate -s $_sz /tmp/$object_key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$object_key s3://\$BUCKET_NAME/$object_key && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/$object_key /tmp/${object_key}_dl && stat -c%s /tmp/${object_key}_dl"
            exit 0
        fi

        if $BINARY cmp "$original_file" "$download_file" 2>/dev/null; then
            echo "PASS:Downloaded and verified $object_key (size: $download_size bytes, content: identical)"
        else
            echo "FAIL:Data integrity check failed for $object_key"
            echo "RERUN:truncate -s $_sz /tmp/$object_key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$object_key s3://\$BUCKET_NAME/$object_key && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/$object_key /tmp/${object_key}_dl && cmp /tmp/$object_key /tmp/${object_key}_dl"
            exit 0
        fi

        # Single stat call retrieves ETag, Content-Length, and SHA-256 checksum together.
        stat_output=$($BINARY stat "s3://$BUCKET_NAME/$object_key" 2>/dev/null)
        response_etag=$(echo "$stat_output" | grep "^ETag" | sed 's/ETag *: //' | tr -d '"')
        response_content_length=$(echo "$stat_output" | grep "^Size" | sed 's/Size *: //; s/ bytes.*//')
        response_checksum=$(echo "$stat_output" | grep "^SHA256" | awk '{print $3}')

        # Check ETag header — S3 Express One Zone uses random/opaque ETags by design;
        # any non-empty ETag is valid (multipart ETags contain "-").
        if [ -n "$response_etag" ]; then
            if [[ "$response_etag" == *"-"* ]]; then
                echo "PASS:Response ETag (multipart): $response_etag"
            else
                echo "PASS:Response ETag present: $response_etag"
            fi
        else
            echo "FAIL:Response ETag not found for $object_key"
            echo "RERUN:\$BINARY stat s3://\$BUCKET_NAME/$object_key"
        fi

        # Verify SHA-256 checksum if the server returned one (requires upload with --checksum SHA256).
        # Absence is not a failure — most uploads omit it and integrity is already covered by hsc cmp above.
        if [ -n "$response_checksum" ]; then
            expected_checksum=$(openssl dgst -sha256 -binary "$original_file" | base64)
            if [ "$response_checksum" = "$expected_checksum" ]; then
                echo "PASS:SHA-256 checksum verified: $response_checksum"
            else
                echo "FAIL:SHA-256 checksum mismatch (expected: $expected_checksum, got: $response_checksum)"
                echo "RERUN:\$BINARY stat s3://\$BUCKET_NAME/$object_key"
            fi
        else
            echo "INFO:SHA-256 checksum not returned by server for $object_key (skipped)"
        fi

        # Check Content-Length header
        if [ -n "$response_content_length" ]; then
            if [ "$response_content_length" -eq "$original_size" ]; then
                echo "PASS:Response Content-Length correct: $response_content_length"
            else
                echo "FAIL:Response Content-Length mismatch (expected: $original_size, got: $response_content_length)"
                echo "RERUN:\$BINARY stat s3://\$BUCKET_NAME/$object_key"
            fi
        else
            echo "FAIL:Response Content-Length not found for $object_key"
            echo "RERUN:\$BINARY stat s3://\$BUCKET_NAME/$object_key"
        fi
    ) > "$RESULTS_DIR/job_${_JOB}" &
    ((_JOB++))
done
wait
collect_results

# Step 4: Test range requests with integrity verification using hsc cmp
step_time
echo ""
info "Step 4: Testing range requests and verifying data integrity with 'hsc cmp'..."

# check_range <ok_msg> <fail_msg> <original_file> <range_spec> <s3_uri>
# Runs hsc cmp --range; calls success or error (with rerun command) directly.
check_range() {
    local ok_msg=$1 fail_msg=$2 orig=$3 range=$4 s3uri=$5
    local _key="${s3uri##*/}"
    local _sz; _sz=$(stat -c%s "$orig" 2>/dev/null || echo 0)
    # shellcheck disable=SC2086
    if $BINARY cmp $SSE_DOWNLOAD_ARGS --range "$range" "$orig" "$s3uri" 2>/dev/null; then
        success "$ok_msg"
    else
        error "$fail_msg" \
            "truncate -s ${_sz} /tmp/${_key} && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/${_key} s3://\$BUCKET_NAME/${_key} && \$BINARY cmp $SSE_DOWNLOAD_ARGS --range \"$range\" /tmp/${_key} s3://\$BUCKET_NAME/${_key}"
    fi
}

# verify_range kept for backward compatibility (used as a plain boolean by callers)
verify_range() {
    local original_file=$1
    local range_spec=$2
    local s3_uri=$3

    # shellcheck disable=SC2086
    if $BINARY cmp $SSE_DOWNLOAD_ARGS --range "$range_spec" "$original_file" "$s3_uri" 2>/dev/null; then
        return 0
    else
        return 1
    fi
}

# Test different ranges on 1m file
test_ranges=("bytes=0-1023" "bytes=1024-2047" "bytes=0-511" "bytes=512000-1048575")
for range in "${test_ranges[@]}"; do
    original_file="$TEST_DIR/testfile_1m.dat"
    info "Verifying testfile_1m.dat range: $range..."
    check_range "Range verified: $range" "Range integrity failed: $range" \
        "$original_file" "$range" "s3://$BUCKET_NAME/testfile_1m.dat"
done

info "Testing range on large file (64m)..."
check_range "Range on 64m file verified: bytes=0-1048575 (1MB)" \
    "Range on 64m file integrity failed: bytes=0-1048575" \
    "$TEST_DIR/testfile_64m.dat" "bytes=0-1048575" "s3://$BUCKET_NAME/testfile_64m.dat"

info "Testing middle range on 8m file..."
check_range "Middle range on 8m file verified: bytes=4194304-5242879" \
    "Middle range on 8m file integrity failed: bytes=4194304-5242879" \
    "$TEST_DIR/testfile_8m.dat" "bytes=4194304-5242879" "s3://$BUCKET_NAME/testfile_8m.dat"

info "Testing last 1KB of 32m file..."
check_range "Last 1KB of 32m file verified: bytes=33553408-33554431" \
    "Last 1KB of 32m file integrity failed: bytes=33553408-33554431" \
    "$TEST_DIR/testfile_32m.dat" "bytes=33553408-33554431" "s3://$BUCKET_NAME/testfile_32m.dat"

echo ""
info "Testing range requests on multipart uploaded objects..."

info "Testing ranges on multipart object with 1m parts (3MB total)..."
check_range "Multipart 1m: First half of part 1 verified" \
    "Multipart 1m: First half of part 1 integrity failed" \
    "$TEST_DIR/multipart/multipart_1m_parts.dat" "bytes=0-524287" \
    "s3://$BUCKET_NAME/multipart_1m_parts.dat"
info "  CRITICAL: Range across part 1->2 boundary"
check_range "Multipart 1m: Range across part boundary (part 1->2) verified" \
    "Multipart 1m: Range across part boundary integrity failed" \
    "$TEST_DIR/multipart/multipart_1m_parts.dat" "bytes=1048000-1049599" \
    "s3://$BUCKET_NAME/multipart_1m_parts.dat"
check_range "Multipart 1m: Middle of part 2 verified" \
    "Multipart 1m: Middle of part 2 integrity failed" \
    "$TEST_DIR/multipart/multipart_1m_parts.dat" "bytes=1572864-2097151" \
    "s3://$BUCKET_NAME/multipart_1m_parts.dat"

info "Testing ranges on multipart object with 16m parts (48MB total)..."
check_range "Multipart 16m: First 8MB of part 1 verified" \
    "Multipart 16m: First 8MB integrity failed" \
    "$TEST_DIR/multipart/multipart_16m_parts.dat" "bytes=0-8388607" \
    "s3://$BUCKET_NAME/multipart_16m_parts.dat"
info "  CRITICAL: Range across 16MB part boundary"
check_range "Multipart 16m: Range across part boundary (16MB boundary) verified" \
    "Multipart 16m: Range across part boundary integrity failed" \
    "$TEST_DIR/multipart/multipart_16m_parts.dat" "bytes=16776192-16778239" \
    "s3://$BUCKET_NAME/multipart_16m_parts.dat"
check_range "Multipart 16m: Range in part 3 verified" \
    "Multipart 16m: Range in part 3 integrity failed" \
    "$TEST_DIR/multipart/multipart_16m_parts.dat" "bytes=40000000-41000000" \
    "s3://$BUCKET_NAME/multipart_16m_parts.dat"

info "Testing ranges on multipart object with 32m parts (96MB total)..."
check_range "Multipart 32m: End of part 1 verified" \
    "Multipart 32m: End of part 1 integrity failed" \
    "$TEST_DIR/multipart/multipart_32m_parts.dat" "bytes=33554000-33554431" \
    "s3://$BUCKET_NAME/multipart_32m_parts.dat"
info "  CRITICAL: Range across part 2->3 boundary (64MB mark)"
check_range "Multipart 32m: Range across part 2->3 boundary (64MB) verified" \
    "Multipart 32m: Range across part 2->3 boundary integrity failed" \
    "$TEST_DIR/multipart/multipart_32m_parts.dat" "bytes=67108000-67109000" \
    "s3://$BUCKET_NAME/multipart_32m_parts.dat"
info "  CRITICAL: Large range spanning all 3 parts (80MB)"
check_range "Multipart 32m: Large range spanning all parts verified (80MB)" \
    "Multipart 32m: Large range spanning all parts integrity failed" \
    "$TEST_DIR/multipart/multipart_32m_parts.dat" "bytes=10000000-90000000" \
    "s3://$BUCKET_NAME/multipart_32m_parts.dat"

# Step 5: Chunk boundary tests — putObject / getObject
step_time
echo ""
CHUNK_SIZE=${CHUNK_SIZE:-4194304}   # 4 MB default; override with: CHUNK_SIZE=<bytes>
C=$CHUNK_SIZE
C2=$((CHUNK_SIZE * 2))
C3=$((CHUNK_SIZE * 3))
info "Step 5: Chunk boundary putObject/getObject (CHUNK_SIZE=${CHUNK_SIZE} bytes)..."

# Test object sizes: ±1 byte around each of the 1-, 2-, and 3-chunk boundaries
CB_LABELS=("chunk1_minus1" "chunk1_exact" "chunk1_plus1"
           "chunk2_minus1" "chunk2_exact" "chunk2_plus1"
           "chunk3_exact")
CB_BYTES=($((C-1)) $C $((C+1)) $((C2-1)) $C2 $((C2+1)) $C3)

mkdir -p "$TEST_DIR/chunk_boundary" "$TEST_DIR/chunk_downloads"

# Create all chunk-boundary files instantly with truncate (sparse, zero-filled)
for i in "${!CB_LABELS[@]}"; do
    truncate -s "${CB_BYTES[$i]}" "$TEST_DIR/chunk_boundary/cb_${CB_LABELS[$i]}.dat"
done
info "Chunk-boundary test files created (${#CB_LABELS[@]} files)"

# Step 5a: putObject — upload all chunk-boundary files at once via sync
echo ""
info "Step 5a: putObject — uploading chunk-boundary files..."
# shellcheck disable=SC2086
if $BINARY sync $SSE_UPLOAD_ARGS "$TEST_DIR/chunk_boundary/" "s3://$BUCKET_NAME/" 2>/dev/null; then
    success "putObject: uploaded ${#CB_LABELS[@]} chunk-boundary files"
else
    error "putObject: sync failed for chunk-boundary files"
fi

# Step 5b: getObject — download and byte-verify every chunk-boundary file in parallel
echo ""
info "Step 5b: getObject — downloading and verifying chunk-boundary files..."
_JOB=0
for i in "${!CB_LABELS[@]}"; do
    label=${CB_LABELS[$i]}; size=${CB_BYTES[$i]}
    (
        orig="$TEST_DIR/chunk_boundary/cb_${label}.dat"
        dl="$TEST_DIR/chunk_downloads/cb_${label}.dat"
        echo "INFO:getObject cb_${label}..."
        if ! $BINARY cp $SSE_DOWNLOAD_ARGS "s3://$BUCKET_NAME/cb_${label}.dat" "$dl" >/dev/null 2>&1; then
            echo "FAIL:getObject failed for cb_${label}"
            echo "RERUN:truncate -s ${size} /tmp/cb_${label}.dat && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/cb_${label}.dat s3://\$BUCKET_NAME/cb_${label}.dat && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/cb_${label}.dat /tmp/cb_${label}_dl.dat"
            exit 0
        fi
        actual=$(stat -c%s "$dl")
        if [ "$actual" -ne "$size" ]; then
            echo "FAIL:getObject size mismatch cb_${label}: expected ${size}, got ${actual}"
            echo "RERUN:truncate -s ${size} /tmp/cb_${label}.dat && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/cb_${label}.dat s3://\$BUCKET_NAME/cb_${label}.dat && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/cb_${label}.dat /tmp/cb_${label}_dl.dat && stat -c%s /tmp/cb_${label}_dl.dat"
            exit 0
        fi
        if $BINARY cmp "$orig" "$dl" 2>/dev/null; then
            echo "PASS:getObject cb_${label} (${size} bytes, content identical)"
        else
            echo "FAIL:getObject data integrity failed for cb_${label}"
            echo "RERUN:truncate -s ${size} /tmp/cb_${label}.dat && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/cb_${label}.dat s3://\$BUCKET_NAME/cb_${label}.dat && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/cb_${label}.dat /tmp/cb_${label}_dl.dat && cmp /tmp/cb_${label}.dat /tmp/cb_${label}_dl.dat"
        fi
    ) > "$RESULTS_DIR/job_${_JOB}" &
    ((_JOB++))
done
wait
collect_results

# Step 5c: getObjectRange — ±4-byte ranges targeting every chunk boundary
step_time
echo ""
info "Step 5c: getObjectRange — ±4 bytes at every chunk boundary..."

_cmp_range() {
    local label=$1 range=$2
    local orig="$TEST_DIR/chunk_boundary/cb_${label}.dat"
    local s3uri="s3://$BUCKET_NAME/cb_${label}.dat"
    local _sz; _sz=$(stat -c%s "$orig" 2>/dev/null || echo 0)
    # shellcheck disable=SC2086
    if $BINARY cmp $SSE_DOWNLOAD_ARGS --range "$range" "$orig" "$s3uri" 2>/dev/null; then
        success "getObjectRange [cb_${label}] $range"
    else
        error "getObjectRange [cb_${label}] $range — FAILED" \
              "truncate -s ${_sz} /tmp/cb_${label}.dat && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/cb_${label}.dat s3://\$BUCKET_NAME/cb_${label}.dat && \$BINARY cmp $SSE_DOWNLOAD_ARGS --range \"$range\" /tmp/cb_${label}.dat s3://\$BUCKET_NAME/cb_${label}.dat"
    fi
}

# chunk1_exact (size=C): object fills exactly one chunk; test boundary edge bytes
info "  [chunk1_exact size=${C}]"
_cmp_range chunk1_exact "bytes=0-0"
_cmp_range chunk1_exact "bytes=$((C-1))-$((C-1))"
_cmp_range chunk1_exact "bytes=$((C/2))-$((C-1))"

# chunk1_plus1 (size=C+1): 1 byte spills into 2nd chunk — straddle chunk1 boundary
info "  [chunk1_plus1 size=$((C+1))] — chunk1 boundary straddle"
_cmp_range chunk1_plus1 "bytes=$((C-4))-$((C-1))"    # last 4 bytes of chunk1
_cmp_range chunk1_plus1 "bytes=$((C))-$((C))"         # only byte in chunk2
_cmp_range chunk1_plus1 "bytes=$((C-1))-$((C))"       # 1 byte each side of boundary
_cmp_range chunk1_plus1 "bytes=$((C-4))-$((C))"       # 5 bytes crossing boundary

# chunk2_minus1 (size=2C-1): ends 1 byte before the 2nd chunk boundary
info "  [chunk2_minus1 size=$((C2-1))]"
_cmp_range chunk2_minus1 "bytes=$((C-4))-$((C+3))"    # cross chunk1→2 boundary
_cmp_range chunk2_minus1 "bytes=$((C2-5))-$((C2-2))"  # last 4 bytes of object

# chunk2_exact (size=2C): two full chunks
info "  [chunk2_exact size=${C2}]"
_cmp_range chunk2_exact "bytes=$((C-4))-$((C+3))"     # cross chunk1→2 boundary
_cmp_range chunk2_exact "bytes=$((C-1))-$((C))"        # single-byte straddle
_cmp_range chunk2_exact "bytes=$((C2-4))-$((C2-1))"   # last 4 bytes
_cmp_range chunk2_exact "bytes=0-$((C2-1))"            # full object

# chunk2_plus1 (size=2C+1): 1 byte spills into 3rd chunk — straddle chunk2 boundary
info "  [chunk2_plus1 size=$((C2+1))] — chunk2 boundary straddle"
_cmp_range chunk2_plus1 "bytes=$((C-4))-$((C+3))"     # cross chunk1→2
_cmp_range chunk2_plus1 "bytes=$((C2-1))-$((C2))"     # 1 byte each side of chunk2
_cmp_range chunk2_plus1 "bytes=$((C2-4))-$((C2))"     # 5 bytes crossing chunk2→3
_cmp_range chunk2_plus1 "bytes=$((C2))-$((C2))"        # only byte in chunk3

# chunk3_exact (size=3C): three full chunks — every boundary exercised
info "  [chunk3_exact size=${C3}] — all boundaries"
_cmp_range chunk3_exact "bytes=$((C-4))-$((C+3))"      # chunk1→2
_cmp_range chunk3_exact "bytes=$((C-1))-$((C))"         # chunk1→2 single-byte straddle
_cmp_range chunk3_exact "bytes=$((C2-4))-$((C2+3))"    # chunk2→3
_cmp_range chunk3_exact "bytes=$((C2-1))-$((C2))"       # chunk2→3 single-byte straddle
_cmp_range chunk3_exact "bytes=$((C3-4))-$((C3-1))"    # last 4 bytes of object
_cmp_range chunk3_exact "bytes=$((C/2))-$((C2+C/2-1))" # large range spanning all boundaries

# Step 6: uploadPart — multipart with part sizes that cross storage chunk boundaries
step_time
echo ""
info "Step 6: uploadPart — multipart upload with chunk-misaligned part sizes..."
info "  Part sizes 5M / 6M / 7M: none is a multiple of the ${CHUNK_SIZE}-byte storage chunk"

mkdir -p "$TEST_DIR/multipart_chunk"
MPC_SIZES=("5m" "6m" "7m")   # 3 parts each; intentionally misaligned with 4M chunks

_JOB=0
for part_size in "${MPC_SIZES[@]}"; do
    (
        part_bytes=$((${part_size%m} * 1048576))
        total=$((part_bytes * 3))
        combined="$TEST_DIR/multipart_chunk/mpc_${part_size}x3.dat"
        key="mpc_${part_size}x3.dat"

        echo "INFO:Creating ${part_size}×3 file (${total} bytes)..."
        truncate -s "$total" "$combined"

        echo "INFO:uploadPart $key (${total} bytes)..."
        if ! $BINARY cp $SSE_UPLOAD_ARGS "$combined" "s3://$BUCKET_NAME/$key" >/dev/null 2>&1; then
            echo "FAIL:uploadPart failed for $key"
            echo "RERUN:truncate -s $total /tmp/$key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$key s3://\$BUCKET_NAME/$key"
            exit 0
        fi
        echo "PASS:uploadPart $key (${total} bytes)"

        if ! _hsc_cmp "$combined" "s3://$BUCKET_NAME/$key"; then
            echo "FAIL:uploadPart full-object integrity failed for $key"
            echo "RERUN:truncate -s $total /tmp/$key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$key s3://\$BUCKET_NAME/$key && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/$key /tmp/${key}_dl && cmp /tmp/$key /tmp/${key}_dl"
            exit 0
        fi
        echo "PASS:uploadPart full-object integrity verified for $key"

        # getObjectRange at every storage chunk boundary within the uploaded object
        num_chunks=$(( (total + C - 1) / C ))
        for (( ci=1; ci < num_chunks; ci++ )); do
            boundary=$((ci * C))
            range="bytes=$((boundary - 4))-$((boundary + 3))"
            if [[ -n "$SSE_DOWNLOAD_ARGS" ]]; then
                echo "INFO:Range check skipped (SSE-C): $key chunk${ci}→$((ci+1)) boundary $range"
            elif $BINARY cmp --range "$range" "$combined" "s3://$BUCKET_NAME/$key" 2>/dev/null; then
                echo "PASS:getObjectRange $key chunk${ci}→$((ci+1)) boundary $range"
            else
                echo "FAIL:getObjectRange $key chunk${ci}→$((ci+1)) boundary FAILED $range"
                echo "RERUN:truncate -s $total /tmp/$key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$key s3://\$BUCKET_NAME/$key && \$BINARY cmp --range \"$range\" /tmp/$key s3://\$BUCKET_NAME/$key"
            fi
        done
    ) > "$RESULTS_DIR/job_${_JOB}" &
    ((_JOB++))
done
wait
collect_results

# Step 7: copyObject — server-side copy at sub-chunk, exact-chunk, and multi-chunk sizes
step_time
echo ""
info "Step 7: copyObject — server-side copy at chunk boundaries..."
mkdir -p "$TEST_DIR/copy_verify"

COPY_SRCS=("cb_chunk1_minus1.dat" "cb_chunk1_exact.dat" "cb_chunk1_plus1.dat"
           "cb_chunk3_exact.dat"   "mpc_5mx3.dat")
COPY_DSTS=("cp_sub_chunk.dat"      "cp_exact_chunk.dat"   "cp_cross_chunk.dat"
           "cp_three_chunks.dat"    "cp_multipart_5m.dat")
COPY_ORIGS=("$TEST_DIR/chunk_boundary/cb_chunk1_minus1.dat"
            "$TEST_DIR/chunk_boundary/cb_chunk1_exact.dat"
            "$TEST_DIR/chunk_boundary/cb_chunk1_plus1.dat"
            "$TEST_DIR/chunk_boundary/cb_chunk3_exact.dat"
            "$TEST_DIR/multipart_chunk/mpc_5mx3.dat")

_JOB=0
for i in "${!COPY_SRCS[@]}"; do
    (
        src=${COPY_SRCS[$i]}; dst=${COPY_DSTS[$i]}; orig=${COPY_ORIGS[$i]}
        echo "INFO:copyObject $src → $dst..."
        _src_sz=$(stat -c%s "$orig" 2>/dev/null || echo 0)
        if ! $BINARY cp $SSE_COPY_ARGS "s3://$BUCKET_NAME/$src" "s3://$BUCKET_NAME/$dst" >/dev/null 2>&1; then
            echo "FAIL:copyObject failed: $src → $dst"
            echo "RERUN:truncate -s $_src_sz /tmp/$src && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$src s3://\$BUCKET_NAME/$src && \$BINARY cp $SSE_COPY_ARGS s3://\$BUCKET_NAME/$src s3://\$BUCKET_NAME/$dst"
            exit 0
        fi
        echo "PASS:copyObject $src → $dst"
        # Download the copy and byte-verify against the local original
        dl="$TEST_DIR/copy_verify/$dst"
        if $BINARY cp $SSE_DOWNLOAD_ARGS "s3://$BUCKET_NAME/$dst" "$dl" >/dev/null 2>&1 \
                && $BINARY cmp "$orig" "$dl" 2>/dev/null; then
            echo "PASS:copyObject integrity verified: $dst matches $src"
        else
            echo "FAIL:copyObject integrity failed: $dst does not match $src"
            echo "RERUN:truncate -s $_src_sz /tmp/$src && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$src s3://\$BUCKET_NAME/$src && \$BINARY cp $SSE_COPY_ARGS s3://\$BUCKET_NAME/$src s3://\$BUCKET_NAME/$dst && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/$dst /tmp/$dst && cmp /tmp/$src /tmp/$dst"
        fi
    ) > "$RESULTS_DIR/job_${_JOB}" &
    ((_JOB++))
done
wait
collect_results

# Step 8: EC stripe boundary tests
#
# Each CHUNK_SIZE-byte chunk is stored under one of three storage policies:
#   3-replica : full chunk replicated 3×  (no sub-chunk striping)
#   EC 2+1    : chunk split into 2 data stripes  → stripe = CHUNK_SIZE / 2
#   EC 4+2    : chunk split into 4 data stripes  → stripe = CHUNK_SIZE / 4
#
# Unique intra-chunk stripe boundaries (for default 4 MB chunk):
#   EC 4+2 only  :  C/4  and  3C/4   (1 MB and 3 MB)
#   EC 2+1 only  :  C/2              (2 MB)
#   chunk        :  C                (shared by all policies)
#
# Tests probe every combination of the form  k·stripe ± {0,1}  for both stripe
# sizes, across the first two storage chunks, plus a large 2-chunk+stripe object.
step_time
echo ""
S21=$((CHUNK_SIZE / 2))   # EC 2+1 data stripe size
S42=$((CHUNK_SIZE / 4))   # EC 4+2 data stripe size
info "Step 8: EC stripe boundary tests  (CHUNK=${CHUNK_SIZE}B  S42=${S42}B  S21=${S21}B)"
mkdir -p "$TEST_DIR/ec" "$TEST_DIR/ec_dl" "$TEST_DIR/ec_copy" "$TEST_DIR/ec_mp"

# ── 8a: putObject / getObject at every EC stripe-derived size ────────────────
echo ""
info "Step 8a: putObject / getObject at EC stripe-boundary sizes..."

# Labels and byte sizes.
# Sizes already covered by Step 5 (C±1, 2C±1, 3C) are excluded.
# Covered boundaries per group:
#   s42*    → EC4+2 stripe-1 (C/4)
#   s42x3*  → EC4+2 stripe-3 (3C/4)   ← last stripe before chunk boundary
#   s21*    → EC2+1 stripe   (C/2)     ← also EC4+2 stripe-2
#   c_s42*  → EC4+2 stripe-1 inside chunk-2
#   c_s21*  → EC2+1 stripe   inside chunk-2
#   c_s42x3*→ EC4+2 stripe-3 inside chunk-2
#   c2_*    → large 2-chunk objects spanning every boundary above
EC_LABELS=(
    "s42_m1"      "s42"      "s42_p1"        # EC4+2 stripe-1 boundary
    "s42x3_m1"    "s42x3"    "s42x3_p1"      # EC4+2 stripe-3 boundary
    "s21_m1"      "s21"      "s21_p1"        # EC2+1 stripe boundary
    "c_s42_m1"    "c_s42"    "c_s42_p1"      # EC4+2 stripe-1 inside chunk-2
    "c_s21_m1"    "c_s21"    "c_s21_p1"      # EC2+1 stripe inside chunk-2
    "c_s42x3_m1"  "c_s42x3"  "c_s42x3_p1"   # EC4+2 stripe-3 inside chunk-2
    "c2_s42"      "c2_s21"                   # 2-chunk + stripe (large objects)
)
EC_BYTES=(
    $((S42-1))        $S42          $((S42+1))
    $((3*S42-1))      $((3*S42))    $((3*S42+1))
    $((S21-1))        $S21          $((S21+1))
    $((C+S42-1))      $((C+S42))    $((C+S42+1))
    $((C+S21-1))      $((C+S21))    $((C+S21+1))
    $((C+3*S42-1))    $((C+3*S42))  $((C+3*S42+1))
    $((2*C+S42))      $((2*C+S21))
)

# Create all EC test files in parallel (sparse zero-filled)
for i in "${!EC_LABELS[@]}"; do
    truncate -s "${EC_BYTES[$i]}" "$TEST_DIR/ec/ec_${EC_LABELS[$i]}.dat" &
done
wait
info "EC stripe test files created (${#EC_LABELS[@]} files)"

# Upload all via sync
# shellcheck disable=SC2086
if $BINARY sync $SSE_UPLOAD_ARGS "$TEST_DIR/ec/" "s3://$BUCKET_NAME/" 2>/dev/null; then
    success "putObject: uploaded ${#EC_LABELS[@]} EC stripe files"
else
    error "putObject: sync failed for EC stripe files"
fi

# Download + byte-verify all in parallel
echo ""
info "Step 8a getObject: downloading and verifying EC stripe objects..."
_JOB=0
for i in "${!EC_LABELS[@]}"; do
    label=${EC_LABELS[$i]}; size=${EC_BYTES[$i]}
    (
        orig="$TEST_DIR/ec/ec_${label}.dat"
        dl="$TEST_DIR/ec_dl/ec_${label}.dat"
        echo "INFO:getObject ec_${label}..."
        if ! $BINARY cp $SSE_DOWNLOAD_ARGS "s3://$BUCKET_NAME/ec_${label}.dat" "$dl" >/dev/null 2>&1; then
            echo "FAIL:getObject failed ec_${label}"
            echo "RERUN:truncate -s ${size} /tmp/ec_${label}.dat && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/ec_${label}.dat s3://\$BUCKET_NAME/ec_${label}.dat && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/ec_${label}.dat /tmp/ec_${label}_dl.dat"
            exit 0
        fi
        actual=$(stat -c%s "$dl")
        if [ "$actual" -ne "$size" ]; then
            echo "FAIL:getObject size mismatch ec_${label}: expected ${size} got ${actual}"
            echo "RERUN:truncate -s ${size} /tmp/ec_${label}.dat && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/ec_${label}.dat s3://\$BUCKET_NAME/ec_${label}.dat && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/ec_${label}.dat /tmp/ec_${label}_dl.dat && stat -c%s /tmp/ec_${label}_dl.dat"
            exit 0
        fi
        if $BINARY cmp "$orig" "$dl" 2>/dev/null; then
            echo "PASS:getObject ec_${label} (${size}B content identical)"
        else
            echo "FAIL:getObject data integrity failed ec_${label}"
            echo "RERUN:truncate -s ${size} /tmp/ec_${label}.dat && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/ec_${label}.dat s3://\$BUCKET_NAME/ec_${label}.dat && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/ec_${label}.dat /tmp/ec_${label}_dl.dat && cmp /tmp/ec_${label}.dat /tmp/ec_${label}_dl.dat"
        fi
    ) > "$RESULTS_DIR/job_${_JOB}" &
    ((_JOB++))
done
wait
collect_results

# ── 8b: getObjectRange at every EC stripe boundary ────────────────────────────
step_time
echo ""
info "Step 8b: getObjectRange at every EC stripe boundary..."

# _ec_range <label> <range> — compare a range against the already-uploaded object.
_ec_range() {
    local label=$1 range=$2
    local orig="$TEST_DIR/ec/ec_${label}.dat"
    local s3uri="s3://$BUCKET_NAME/ec_${label}.dat"
    local _sz; _sz=$(stat -c%s "$orig" 2>/dev/null || echo 0)
    # shellcheck disable=SC2086
    if $BINARY cmp $SSE_DOWNLOAD_ARGS --range "$range" "$orig" "$s3uri" 2>/dev/null; then
        success "getObjectRange [ec_${label}] $range"
    else
        error "getObjectRange [ec_${label}] $range — FAILED" \
              "truncate -s ${_sz} /tmp/ec_${label}.dat && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/ec_${label}.dat s3://\$BUCKET_NAME/ec_${label}.dat && \$BINARY cmp $SSE_DOWNLOAD_ARGS --range \"$range\" /tmp/ec_${label}.dat s3://\$BUCKET_NAME/ec_${label}.dat"
    fi
}

# EC4+2 stripe-1 boundary (offset = S42) — object size must be > S42
info "  EC4+2 stripe-1 @ offset ${S42} (policy boundary: C/4)"
_ec_range "s42_p1"     "bytes=$((S42-1))-$((S42))"           # 2B straddle
_ec_range "s42_p1"     "bytes=$((S42-4))-$((S42))"           # 4B before + boundary

# EC4+2 stripe-3 boundary (offset = 3*S42) — last stripe edge before chunk end
info "  EC4+2 stripe-3 @ offset $((3*S42)) (policy boundary: 3C/4)"
_ec_range "s42x3_p1"   "bytes=$((3*S42-1))-$((3*S42))"       # 2B straddle
_ec_range "s42x3_p1"   "bytes=$((3*S42-4))-$((3*S42))"       # 4B before

# EC2+1 stripe boundary (offset = S21 = 2*S42)
info "  EC2+1 stripe   @ offset ${S21} (policy boundary: C/2)"
_ec_range "s21_p1"     "bytes=$((S21-1))-$((S21))"            # 2B straddle
_ec_range "s21_p1"     "bytes=$((S21-4))-$((S21))"
_ec_range "s21_p1"     "bytes=0-$((S21))"                     # first stripe + 1B

# Inside chunk-2: EC4+2 stripe-1 (offset = C+S42)
info "  EC4+2 stripe-1 in chunk-2 @ offset $((C+S42))"
_ec_range "c_s42_p1"   "bytes=$((C+S42-1))-$((C+S42))"       # 2B straddle
_ec_range "c_s42_p1"   "bytes=$((C+S42-4))-$((C+S42))"
_ec_range "c_s42_p1"   "bytes=$((C-1))-$((C+S42))"            # chunk boundary → stripe

# Inside chunk-2: EC2+1 stripe (offset = C+S21)
info "  EC2+1 stripe   in chunk-2 @ offset $((C+S21))"
_ec_range "c_s21_p1"   "bytes=$((C+S21-1))-$((C+S21))"       # 2B straddle
_ec_range "c_s21_p1"   "bytes=$((C+S21-4))-$((C+S21))"
_ec_range "c_s21_p1"   "bytes=$((C-4))-$((C+S21))"            # wide: chunk boundary → EC2+1 stripe

# Inside chunk-2: EC4+2 stripe-3 (offset = C+3*S42)
info "  EC4+2 stripe-3 in chunk-2 @ offset $((C+3*S42))"
_ec_range "c_s42x3_p1" "bytes=$((C+3*S42-1))-$((C+3*S42))"   # 2B straddle
_ec_range "c_s42x3_p1" "bytes=$((C+3*S42-4))-$((C+3*S42))"
_ec_range "c_s42x3_p1" "bytes=$((C+S21-4))-$((C+3*S42))"     # EC2+1→EC4+2 within chunk-2

# Large 2-chunk objects: traverse every stripe and chunk boundary in one object
info "  Large object c2_s42 (${#}B): all boundaries"
_ec_range "c2_s42"     "bytes=$((S42-1))-$((S42))"            # EC4+2 in chunk-1
_ec_range "c2_s42"     "bytes=$((3*S42-1))-$((3*S42))"        # EC4+2 stripe-3 in chunk-1
_ec_range "c2_s42"     "bytes=$((S21-1))-$((S21))"            # EC2+1 in chunk-1
_ec_range "c2_s42"     "bytes=$((C-1))-$((C))"                # chunk-1 → chunk-2
_ec_range "c2_s42"     "bytes=$((C+S42-1))-$((C+S42))"        # EC4+2 in chunk-2
_ec_range "c2_s42"     "bytes=$((C+S21-1))-$((C+S21))"        # EC2+1 in chunk-2
_ec_range "c2_s42"     "bytes=$((C+3*S42-1))-$((C+3*S42))"   # EC4+2 stripe-3 in chunk-2
_ec_range "c2_s42"     "bytes=$((2*C-1))-$((2*C+S42-1))"      # chunk-2 → chunk-3 + full stripe

info "  Large object c2_s21 (${#}B): all boundaries"
_ec_range "c2_s21"     "bytes=$((S42-1))-$((S42))"
_ec_range "c2_s21"     "bytes=$((S21-1))-$((S21))"
_ec_range "c2_s21"     "bytes=$((C-1))-$((C))"                 # chunk boundary
_ec_range "c2_s21"     "bytes=$((C+S21-1))-$((C+S21))"
_ec_range "c2_s21"     "bytes=$((2*C-1))-$((2*C+S21-1))"       # chunk-2→3 + entire EC2+1 stripe
_ec_range "c2_s21"     "bytes=0-$((2*C+S21-1))"                # full object

# ── 8c: uploadPart — part sizes misaligned to EC stripes ─────────────────────
step_time
echo ""
info "Step 8c: uploadPart — part sizes misaligned to EC stripes..."
#
# Part sizes chosen so that every uploadPart boundary (where one part ends and
# the next begins) lands at an offset that is NOT a multiple of either stripe
# size, forcing the server to stitch EC stripes across part boundaries:
#
#   S42+1      : 1B over one EC4+2 stripe   (part ends mid-stripe-2)
#   S21+1      : 1B over one EC2+1 stripe   (part ends mid-stripe-2)
#   3*S42+1    : covers 3 EC4+2 stripes+1B  (part ends 1B into next EC4+2 stripe)
#   C+S42/2    : straddles a chunk boundary AND lands mid-stripe inside chunk-2
#
# Each object is 4 parts; after upload getObjectRange is tested at every
# EC stripe and chunk boundary that falls within the object.
EC_MP_PART_SIZES=($((S42+1))  $((S21+1))  $((3*S42+1))  $((C+S42/2)))
EC_MP_PART_LABELS=("s42p1x4"  "s21p1x4"   "3s42p1x4"    "c_halfS42x4")

_JOB=0
for i in "${!EC_MP_PART_LABELS[@]}"; do
    (
        part_bytes=${EC_MP_PART_SIZES[$i]}
        label=${EC_MP_PART_LABELS[$i]}
        total=$((part_bytes * 4))
        combined="$TEST_DIR/ec_mp/ec_mp_${label}.dat"
        key="ec_mp_${label}.dat"

        echo "INFO:uploadPart ec_mp_${label}: 4×${part_bytes}B = ${total}B..."
        truncate -s "$total" "$combined"

        if ! $BINARY cp $SSE_UPLOAD_ARGS "$combined" "s3://$BUCKET_NAME/$key" >/dev/null 2>&1; then
            echo "FAIL:uploadPart failed ec_mp_${label}"
            echo "RERUN:truncate -s $total /tmp/$key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$key s3://\$BUCKET_NAME/$key"
            exit 0
        fi
        echo "PASS:uploadPart ec_mp_${label} (4×${part_bytes}B = ${total}B)"

        if ! _hsc_cmp "$combined" "s3://$BUCKET_NAME/$key"; then
            echo "FAIL:uploadPart full-object integrity failed ec_mp_${label}"
            echo "RERUN:truncate -s $total /tmp/$key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$key s3://\$BUCKET_NAME/$key && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/$key /tmp/${key}_dl && cmp /tmp/$key /tmp/${key}_dl"
            exit 0
        fi
        echo "PASS:uploadPart full-object integrity ec_mp_${label}"

        # getObjectRange at every EC stripe and chunk boundary within this object.
        # Both stripe sizes are checked; boundaries at or beyond EOF are skipped.
        for stride in $S42 $S21 $C; do
            k=1
            while true; do
                boundary=$((k * stride))
                [ "$boundary" -ge "$total" ] && break
                range="bytes=$((boundary-1))-$((boundary))"
                if [[ -n "$SSE_DOWNLOAD_ARGS" ]]; then
                    echo "INFO:Range check skipped (SSE-C): ec_mp_${label} stride=${stride} @${boundary}"
                elif $BINARY cmp --range "$range" "$combined" \
                        "s3://$BUCKET_NAME/$key" 2>/dev/null; then
                    echo "PASS:getObjectRange ec_mp_${label} stride=${stride} @${boundary} OK"
                else
                    echo "FAIL:getObjectRange ec_mp_${label} stride=${stride} @${boundary} FAILED"
                    echo "RERUN:truncate -s $total /tmp/$key && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$key s3://\$BUCKET_NAME/$key && \$BINARY cmp --range \"$range\" /tmp/$key s3://\$BUCKET_NAME/$key"
                fi
                ((k++))
            done
        done
    ) > "$RESULTS_DIR/job_${_JOB}" &
    ((_JOB++))
done
wait
collect_results

# ── 8d: copyObject at EC stripe sizes ────────────────────────────────────────
step_time
echo ""
info "Step 8d: copyObject — server-side copy at EC stripe sizes..."
#
# Source objects span the full range of stripe cases:
#   ec_s42     : exactly one EC4+2 stripe   (sub-chunk, no EC2+1 boundary)
#   ec_s21     : exactly one EC2+1 stripe   (= two EC4+2 stripes, still sub-chunk)
#   ec_c_s42   : C + one EC4+2 stripe       (cross-chunk, lands mid-EC2+1)
#   ec_c_s21   : C + one EC2+1 stripe       (cross-chunk, fills half of chunk-2)
#   ec_c2_s21  : 2C + one EC2+1 stripe      (3-chunk span, exercises all policies)
EC_COPY_SRCS=("ec_s42.dat"    "ec_s21.dat"    "ec_c_s42.dat"
              "ec_c_s21.dat"  "ec_c2_s21.dat")
EC_COPY_DSTS=("ecc_s42.dat"   "ecc_s21.dat"   "ecc_c_s42.dat"
              "ecc_c_s21.dat" "ecc_c2_s21.dat")
EC_COPY_ORIGS=("$TEST_DIR/ec/ec_s42.dat"    "$TEST_DIR/ec/ec_s21.dat"
               "$TEST_DIR/ec/ec_c_s42.dat"  "$TEST_DIR/ec/ec_c_s21.dat"
               "$TEST_DIR/ec/ec_c2_s21.dat")

_JOB=0
for i in "${!EC_COPY_SRCS[@]}"; do
    (
        src=${EC_COPY_SRCS[$i]}; dst=${EC_COPY_DSTS[$i]}; orig=${EC_COPY_ORIGS[$i]}
        echo "INFO:copyObject $src → $dst..."
        _src_sz=$(stat -c%s "$orig" 2>/dev/null || echo 0)
        if ! $BINARY cp $SSE_COPY_ARGS "s3://$BUCKET_NAME/$src" "s3://$BUCKET_NAME/$dst" >/dev/null 2>&1; then
            echo "FAIL:copyObject failed $src → $dst"
            echo "RERUN:truncate -s $_src_sz /tmp/$src && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$src s3://\$BUCKET_NAME/$src && \$BINARY cp $SSE_COPY_ARGS s3://\$BUCKET_NAME/$src s3://\$BUCKET_NAME/$dst"
            exit 0
        fi
        echo "PASS:copyObject $src → $dst"
        dl="$TEST_DIR/ec_copy/$dst"
        if $BINARY cp $SSE_DOWNLOAD_ARGS "s3://$BUCKET_NAME/$dst" "$dl" >/dev/null 2>&1 \
                && $BINARY cmp "$orig" "$dl" 2>/dev/null; then
            echo "PASS:copyObject integrity verified $dst"
        else
            echo "FAIL:copyObject integrity failed $dst"
            echo "RERUN:truncate -s $_src_sz /tmp/$src && \$BINARY cp $SSE_UPLOAD_ARGS /tmp/$src s3://\$BUCKET_NAME/$src && \$BINARY cp $SSE_COPY_ARGS s3://\$BUCKET_NAME/$src s3://\$BUCKET_NAME/$dst && \$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/$dst /tmp/$dst && cmp /tmp/$src /tmp/$dst"
        fi
    ) > "$RESULTS_DIR/job_${_JOB}" &
    ((_JOB++))
done
wait
collect_results

# ── Step 8e: SSE-C key validation ────────────────────────────────────────────
# Runs only when HSC_SSE is set (AES256, sse-c, aws:kms).
# Requires the S3 server to support SSE-C.  Skip if SSE is not configured.
# A fresh random 256-bit key is generated; the test verifies that:
#   (a) upload with SSE-C key succeeds
#   (b) download with the correct key succeeds and content is intact
#   (c) download WITHOUT any SSE-C key fails (server should return 400/403)
#   (d) download with a WRONG key fails
step_time
echo ""
_SSEC_OBJ="ssec_validate_test.dat"
if [[ -z "$HSC_SSE" ]]; then
    info "Step 8e: SSE-C key validation skipped (set HSC_SSE=sse-c to enable)"
else
info "Step 8e: SSE-C key validation tests..."
_SSEC_KEY=$(openssl rand -base64 32)
_SSEC_WRONG=$(openssl rand -base64 32)
_SSEC_SIZE=65536
truncate -s $_SSEC_SIZE "$TEST_DIR/$_SSEC_OBJ" 2>/dev/null || dd if=/dev/random of="$TEST_DIR/$_SSEC_OBJ" bs=$_SSEC_SIZE count=1 status=none

# (a) upload with SSE-C key
if $BINARY cp --sse-c AES256 --sse-c-key "$_SSEC_KEY" \
        "$TEST_DIR/$_SSEC_OBJ" "s3://$BUCKET_NAME/$_SSEC_OBJ" >/dev/null 2>&1; then
    success "SSE-C: upload with customer key succeeded"
else
    error "SSE-C: upload with customer key failed"
fi

# (b) download with correct key — must succeed and data must match
_ssec_dl="$TEST_DIR/${_SSEC_OBJ}_dl"
if $BINARY cp --sse-c AES256 --sse-c-key "$_SSEC_KEY" \
        "s3://$BUCKET_NAME/$_SSEC_OBJ" "$_ssec_dl" >/dev/null 2>&1 \
        && cmp -s "$TEST_DIR/$_SSEC_OBJ" "$_ssec_dl"; then
    success "SSE-C: download with correct key succeeded and content is intact"
else
    error "SSE-C: download with correct key failed or content mismatch"
fi
rm -f "$_ssec_dl"

# (c) download WITHOUT SSE-C key — server must reject (non-zero exit)
if $BINARY cp "s3://$BUCKET_NAME/$_SSEC_OBJ" "$_ssec_dl" >/dev/null 2>&1; then
    error "SSE-C: download without key unexpectedly succeeded (expected failure)"
else
    success "SSE-C: download without key correctly rejected by server"
fi
rm -f "$_ssec_dl"

# (d) download with WRONG SSE-C key — server must reject
if $BINARY cp --sse-c AES256 --sse-c-key "$_SSEC_WRONG" \
        "s3://$BUCKET_NAME/$_SSEC_OBJ" "$_ssec_dl" >/dev/null 2>&1; then
    error "SSE-C: download with wrong key unexpectedly succeeded (expected failure)"
else
    success "SSE-C: download with wrong key correctly rejected by server"
fi
rm -f "$_ssec_dl"

# Clean up SSE-C test object
$BINARY rm "s3://$BUCKET_NAME/$_SSEC_OBJ" >/dev/null 2>&1 || true
fi  # end HSC_SSE guard

# ── Step 8f: sync --delete and sync --checksum ────────────────────────────────
step_time
echo ""
info "Step 8f: sync --delete and sync --checksum tests..."
_SYNC_DIR="$TEST_DIR/sync_test"
_SYNC_PREFIX="sync_test"
mkdir -p "$_SYNC_DIR"

# Create 3 small files with random content
dd if=/dev/random of="$_SYNC_DIR/sync_a.dat" bs=4096  count=1 status=none
dd if=/dev/random of="$_SYNC_DIR/sync_b.dat" bs=8192  count=1 status=none
dd if=/dev/random of="$_SYNC_DIR/sync_c.dat" bs=16384 count=1 status=none

# Initial sync: upload all 3 files with --checksum
info "  sync --checksum: uploading 3 files..."
# shellcheck disable=SC2086
_sync_out=$($BINARY sync --checksum $SSE_UPLOAD_ARGS "$_SYNC_DIR/" "s3://$BUCKET_NAME/$_SYNC_PREFIX/" 2>&1)
if [ $? -eq 0 ]; then
    success "sync --checksum: initial sync of 3 files succeeded"
else
    error "sync --checksum: initial sync failed: $_sync_out"
fi

# Verify all 3 objects are present
_sync_count=$($BINARY ls "s3://$BUCKET_NAME/$_SYNC_PREFIX/" 2>/dev/null | grep -c 'sync_[abc]\.dat')
if [ "$_sync_count" -eq 3 ]; then
    success "sync --checksum: all 3 objects present in S3"
else
    error "sync --checksum: expected 3 objects, found $_sync_count"
fi

# Remove one local file and re-sync with --delete
rm -f "$_SYNC_DIR/sync_b.dat"
info "  sync --delete: removed sync_b.dat locally, re-syncing..."
# shellcheck disable=SC2086
if $BINARY sync --delete $SSE_UPLOAD_ARGS "$_SYNC_DIR/" "s3://$BUCKET_NAME/$_SYNC_PREFIX/" >/dev/null 2>&1; then
    success "sync --delete: re-sync succeeded"
else
    error "sync --delete: re-sync failed"
fi

# Verify sync_b.dat was deleted from S3
if $BINARY stat "s3://$BUCKET_NAME/$_SYNC_PREFIX/sync_b.dat" >/dev/null 2>&1; then
    error "sync --delete: sync_b.dat still exists in S3 (should have been deleted)"
else
    success "sync --delete: sync_b.dat correctly removed from S3"
fi

# Verify sync_a.dat and sync_c.dat are still present
_sync_remaining=$($BINARY ls "s3://$BUCKET_NAME/$_SYNC_PREFIX/" 2>/dev/null | grep -c 'sync_[ac]\.dat')
if [ "$_sync_remaining" -eq 2 ]; then
    success "sync --delete: remaining 2 objects (sync_a, sync_c) intact"
else
    error "sync --delete: expected 2 remaining objects, found $_sync_remaining"
fi

# Cleanup sync test objects
$BINARY rm "s3://$BUCKET_NAME/$_SYNC_PREFIX/sync_a.dat" >/dev/null 2>&1 || true
$BINARY rm "s3://$BUCKET_NAME/$_SYNC_PREFIX/sync_c.dat" >/dev/null 2>&1 || true
rm -rf "$_SYNC_DIR"

# ── Step 8g: mv, diff, cat, and ls --versions ─────────────────────────────────
step_time
echo ""
info "Step 8g: mv / diff / cat / ls --versions..."
mkdir -p "$TEST_DIR/mv_verify" "$TEST_DIR/diff_src" "$TEST_DIR/cat_verify"

# ── mv: rename a small S3 object ──────────────────────────────────────────────
_MV_SRC="testfile_64k.dat"
_MV_DST="mv_renamed_64k.dat"
info "  mv: s3 rename $BUCKET_NAME/$_MV_SRC → $_MV_DST"
if $BINARY mv $SSE_COPY_ARGS \
        "s3://$BUCKET_NAME/$_MV_SRC" "s3://$BUCKET_NAME/$_MV_DST" >/dev/null 2>&1; then
    success "mv: object renamed successfully"
    # source must be gone
    if $BINARY stat "s3://$BUCKET_NAME/$_MV_SRC" >/dev/null 2>&1; then
        error "mv: source object still exists after mv"
    else
        success "mv: source object correctly removed"
    fi
    # destination must be reachable and correct size
    _mv_dl="$TEST_DIR/mv_verify/$_MV_DST"
    if $BINARY cp $SSE_DOWNLOAD_ARGS \
            "s3://$BUCKET_NAME/$_MV_DST" "$_mv_dl" >/dev/null 2>&1 \
            && [ "$(stat -c%s "$_mv_dl")" -eq 65536 ]; then
        success "mv: destination object download and size verified (65536 bytes)"
    else
        error "mv: destination download or size check failed"
    fi
else
    error "mv: rename failed"
fi

# ── diff: compare local dir to S3 prefix ──────────────────────────────────────
# Populate diff_src with two files, upload them, then diff — expect no differences.
dd if=/dev/random of="$TEST_DIR/diff_src/diff_a.dat" bs=4096  count=1 status=none
dd if=/dev/random of="$TEST_DIR/diff_src/diff_b.dat" bs=8192  count=1 status=none
$BINARY cp $SSE_UPLOAD_ARGS \
    "$TEST_DIR/diff_src/diff_a.dat" "s3://$BUCKET_NAME/diff_src/diff_a.dat" >/dev/null 2>&1
$BINARY cp $SSE_UPLOAD_ARGS \
    "$TEST_DIR/diff_src/diff_b.dat" "s3://$BUCKET_NAME/diff_src/diff_b.dat" >/dev/null 2>&1
info "  diff: comparing local dir to S3 prefix (expect no differences)..."
_diff_out=$($BINARY diff "$TEST_DIR/diff_src/" "s3://$BUCKET_NAME/diff_src/" 2>/dev/null)
if echo "$_diff_out" | grep -qiE 'only in|differ|mismatch'; then
    error "diff: unexpected differences reported: $_diff_out"
else
    success "diff: no differences between local dir and S3 prefix"
fi
# Introduce a size difference — modify one file locally, expect diff to report it
dd if=/dev/random of="$TEST_DIR/diff_src/diff_extra.dat" bs=1024 count=1 status=none
_diff_out2=$($BINARY diff "$TEST_DIR/diff_src/" "s3://$BUCKET_NAME/diff_src/" 2>/dev/null)
if echo "$_diff_out2" | grep -qiE 'only in.*diff_extra|diff_extra.*only'; then
    success "diff: correctly detected file present locally but not in S3"
else
    error "diff: failed to detect extra local file (got: $_diff_out2)"
fi

# ── cat: read byte ranges from an S3 object ───────────────────────────────────
info "  cat: byte-range reads from testfile_1m.dat..."
_cat_file="$TEST_DIR/cat_verify/range.bin"
# First 1 KB
if $BINARY cat "s3://$BUCKET_NAME/testfile_1m.dat" --offset 0 --size 1024 \
        > "$_cat_file" 2>/dev/null && [ "$(stat -c%s "$_cat_file")" -eq 1024 ]; then
    success "cat: --offset 0 --size 1024 returned 1024 bytes"
else
    error "cat: --offset 0 --size 1024 failed or wrong size"
fi
# Middle range via --range
if $BINARY cat "s3://$BUCKET_NAME/testfile_1m.dat" --range 512000-513023 \
        > "$_cat_file" 2>/dev/null && [ "$(stat -c%s "$_cat_file")" -eq 1024 ]; then
    success "cat: --range 512000-513023 returned 1024 bytes"
else
    error "cat: --range 512000-513023 failed or wrong size"
fi

# ── ls --versions: basic smoke test (no versioning required) ──────────────────
info "  ls --versions: smoke test (header line present)..."
_ver_out=$($BINARY ls --versions "s3://$BUCKET_NAME/" 2>/dev/null | head -5)
if echo "$_ver_out" | grep -q 'VERSION-ID'; then
    success "ls --versions: header line present"
else
    error "ls --versions: missing header line (got: $_ver_out)"
fi

# Cleanup
$BINARY rm "s3://$BUCKET_NAME/$_MV_DST"            >/dev/null 2>&1 || true
$BINARY rm "s3://$BUCKET_NAME/diff_src/diff_a.dat" >/dev/null 2>&1 || true
$BINARY rm "s3://$BUCKET_NAME/diff_src/diff_b.dat" >/dev/null 2>&1 || true

# Step 9: Delete all objects
step_time
echo ""
info "Step 9: Deleting all objects..."
if $BINARY rm --recursive "s3://$BUCKET_NAME/" >/dev/null 2>&1; then
    success "Deleted all objects"
else
    error "Failed to delete all objects"
fi

# Verify bucket is empty
echo ""
info "Verifying bucket is empty..."
object_count=$($BINARY ls "s3://$BUCKET_NAME" | grep -c "^[0-9]" || true)
if [ "$object_count" -eq 0 ]; then
    success "Bucket is empty"
else
    error "Bucket still contains $object_count object(s)"
    $BINARY ls "s3://$BUCKET_NAME"
fi

# Step 10: Delete bucket
step_time
echo ""
if [[ -n "$BUCKET_PROVIDED" ]]; then
    info "Step 10: Skipping bucket deletion (bucket '$BUCKET_NAME' was provided by caller)"
else
    info "Step 10: Deleting bucket '$BUCKET_NAME'..."
    if $BINARY rb "s3://$BUCKET_NAME"; then
        success "Bucket deleted successfully"
    else
        error "Failed to delete bucket"
    fi
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
_total=$((SECONDS - SCRIPT_START))
printf "${BLUE}Total time: %dm %02ds${NC}\n" $((_total / 60)) $((_total % 60))
echo "========================================="

if [ $ERROR_COUNT -eq 0 ]; then
    echo -e "${GREEN}🎉 All tests completed successfully!${NC}"
else
    echo -e "${YELLOW}⚠️  Some tests failed. Please review the output above.${NC}"
    echo ""
    echo "========================================="
    echo -e "${RED}         FAILED TEST CASES               ${NC}"
    echo "========================================="
    for _i in "${!FAILED_TESTS[@]}"; do
        echo -e "${RED}[$((_i+1))] ✗ ${FAILED_TESTS[$_i]}${NC}"
        if [ -n "${FAILED_CMDS[$_i]}" ]; then
            echo -e "    ${BLUE}↳ ${FAILED_CMDS[$_i]}${NC}"
        fi
    done
    echo "========================================="

    # Write a self-contained rerun script so failed tests can be replayed manually.
    _rerun_script="./rerun_failed.sh"
    {
        echo "#!/bin/bash"
        echo "# Auto-generated rerun script — failed tests from s3_functional_test.sh"
        echo "# Edit BUCKET_NAME / BINARY below, then: bash rerun_failed.sh"
        echo "BUCKET_NAME=\"\${BUCKET_NAME:-$BUCKET_NAME}\""
        echo "BINARY=\"\${BINARY:-$BINARY}\""
        echo ""
        echo "set +e"
        echo "\$BINARY mb \"s3://\$BUCKET_NAME\" 2>/dev/null || true"
        echo ""
        for _i in "${!FAILED_TESTS[@]}"; do
            echo "# [$((_i+1))] ${FAILED_TESTS[$_i]}"
            echo "echo \"[$((_i+1))] ${FAILED_TESTS[$_i]}\""
            if [ -n "${FAILED_CMDS[$_i]}" ]; then
                echo "${FAILED_CMDS[$_i]}"
            fi
            echo ""
        done
    } > "$_rerun_script"
    chmod +x "$_rerun_script"
    echo -e "${BLUE}Rerun script written to: $_rerun_script${NC}"
fi
echo "========================================="

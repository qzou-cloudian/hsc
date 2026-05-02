#!/bin/bash

# S3 Functional Test Script
# Tests bucket operations, object upload/download/copy/delete with integrity checks,
# and uses 'hsc test object' for comprehensive range and EC stripe boundary verification.

# Don't exit on error - we want to count all failures
set +e

# When HSC_RDMA is set to a non-false value, build and use the rdma-featured binary.
if [[ -n "$HSC_RDMA" && "$HSC_RDMA" != "false" && "$HSC_RDMA" != "0" ]]; then
    if [[ "$HSC_RDMA" == "cuobj" || "$HSC_RDMA" == "auto" ]]; then
        echo "HSC_RDMA=$HSC_RDMA detected — building with --features cuobj ..."
        cargo build --features cuobj 2>&1 | tail -3

        # Build libs3rdmacuobjclient.so and copy it next to the hsc binary so that
        # the runtime loader finds it via the "exe dir" search path.
        _CUOBJ_SRC="${CUOBJ_SRC:-$(cd "$(dirname "$0")/../.." && pwd)/s3-rdma/providers/cuobj-client}"
        if [[ -d "$_CUOBJ_SRC" ]]; then
            echo "Building libs3rdmacuobjclient.so from $_CUOBJ_SRC ..."
            CUOBJ_ROOT_DIR="${CUOBJ_ROOT_DIR:-/usr/local/cuda}" \
                cargo build --manifest-path "$_CUOBJ_SRC/Cargo.toml" \
                    --target-dir "./target/cuobj-build" 2>&1 | tail -3
            _SO="./target/cuobj-build/debug/libs3rdmacuobjclient.so"
            if [[ -f "$_SO" ]]; then
                cp "$_SO" "./target/debug/"
                echo "Copied libs3rdmacuobjclient.so → ./target/debug/"
            else
                echo "Warning: libs3rdmacuobjclient.so not produced — RDMA may fall back to standard I/O"
            fi
        else
            echo "Warning: cuobj-client source not found at $_CUOBJ_SRC — skipping libs3rdmacuobjclient.so build"
        fi
    else
        echo "HSC_RDMA=$HSC_RDMA detected — building with --features rdma ..."
        cargo build --features rdma 2>&1 | tail -3
    fi
    BINARY="./target/debug/hsc"
else
    BINARY="./target/debug/hsc"
fi
if [[ ! -f "$BINARY" ]]; then
    BINARY="hsc"
fi

# SSE configuration — set HSC_SSE to enable server-side encryption for all tests.
#
#   AES256    — S3-managed AES-256 (transparent to reads; all tests work normally)
#   aws:kms   — AWS KMS encryption (set HSC_SSE_KMS_KEY_ID for a specific key)
#   sse-c     — Customer-provided AES-256 key (set HSC_SSE_C_KEY or auto-generated;
#               'hsc cmp --range' supports SSE-C keys; range-verification steps are
#               skipped in this mode when SSE_DOWNLOAD_ARGS contains a key)
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
ENDPOINT="${AWS_ENDPOINT_URL}"
BUCKET_NAME="${1:-test-bucket-$(date +%s)}"
BUCKET_PROVIDED="${1:+true}"   # non-empty when caller supplied a bucket name
TEST_DIR="./test_data"

# Storage policy of the bucket — controls which structural boundaries
# 'hsc test object' probes in Step 4.
#   ec      (default) — tests chunk, part, and EC stripe boundaries (C/4, C/2, 3C/4)
#   replica           — tests chunk and part boundaries only (no EC stripe tests)
# Override: HSC_POLICY=replica ./s3_functional_test.sh my-bucket
POLICY="${HSC_POLICY:-ec}"

# Server storage chunk size passed to 'hsc test object' (default: 4m).
# Override: CHUNK_SIZE_STR=8m ./s3_functional_test.sh my-bucket
CHUNK_SIZE_STR="${CHUNK_SIZE_STR:-4m}"

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

# Object sizes for base upload/download/metadata tests (Steps 2–3).
# Range, chunk-boundary, and EC stripe integrity is handled by 'hsc test object' in Step 4.
SIZES=("1b" "2b" "10b" "1k" "64k" "1m" "8m" "64m")

echo "========================================="
echo "S3 Functional Test"
echo "Endpoint: $ENDPOINT"
echo "Bucket: $BUCKET_NAME"
echo "SSE Mode: ${HSC_SSE:-none}"
echo "Policy: $POLICY  Chunk: $CHUNK_SIZE_STR"
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

# Step 1: Create bucket
echo ""
info "Step 1: Creating bucket '$BUCKET_NAME'..."
if $BINARY mb --ignore-existing "s3://$BUCKET_NAME"; then
    success "Bucket ready: $BUCKET_NAME"
else
    error "Failed to create bucket $BUCKET_NAME"
    exit 1
fi

# Delete any leftover objects from a previous failed run
_existing=$($BINARY ls "s3://$BUCKET_NAME" 2>/dev/null | grep -c "^[0-9]" || true)
if [ "$_existing" -gt 0 ]; then
    info "Bucket contains $_existing leftover object(s) — cleaning up before test..."
    if $BINARY rm --recursive "s3://$BUCKET_NAME/" >/dev/null 2>&1; then
        success "Bucket cleared"
    else
        error "Failed to clear bucket — aborting"
        exit 1
    fi
fi

# Step 2: Create test files and upload objects
step_time
echo ""
info "Step 2: Creating and uploading base test files..."
for size in "${SIZES[@]}"; do
    filename="$TEST_DIR/testfile_${size}.dat"
    case $size in
        *b) bytes=${size%b} ;;
        *k) bytes=$(( ${size%k} * 1024 )) ;;
        *m) bytes=$(( ${size%m} * 1048576 )) ;;
    esac
    dd if=/dev/urandom of="$filename" bs=65536 count=$(( (bytes + 65535) / 65536 )) 2>/dev/null
    truncate -s "$bytes" "$filename"
done
info "All test files created"

# Upload all objects at once
info "Uploading test files to S3..."
# shellcheck disable=SC2086
if $BINARY sync $SSE_UPLOAD_ARGS "$TEST_DIR/" "s3://$BUCKET_NAME/" 2>/dev/null; then
    success "Uploaded ${#SIZES[@]} test files via sync"
else
    error "Failed to upload test files"
    exit 1
fi

# List objects to verify
echo ""
info "Listing objects in bucket..."
$BINARY ls "s3://$BUCKET_NAME"

# Step 3: Download objects (full size) with integrity verification
step_time
echo ""
info "Step 3: Downloading objects (full size) and verifying data integrity..."
mkdir -p "$TEST_DIR/downloads"
for size in "${SIZES[@]}"; do
    object_key="testfile_${size}.dat"
    download_file="$TEST_DIR/downloads/testfile_${size}.dat"
    original_file="$TEST_DIR/testfile_${size}.dat"
    info "Downloading $object_key..."
    # shellcheck disable=SC2086
    if ! $BINARY cp $SSE_DOWNLOAD_ARGS "s3://$BUCKET_NAME/$object_key" "$download_file" >/dev/null 2>&1; then
        error "Failed to download $object_key" "\$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/$object_key \$TEST_DIR/downloads/${object_key}"
        continue
    fi

    original_size=$(stat -c%s "$original_file")
    download_size=$(stat -c%s "$download_file")

    if [ "$original_size" -ne "$download_size" ]; then
        error "Size mismatch for $object_key (expected: $original_size, got: $download_size)" "\$BINARY cp $SSE_DOWNLOAD_ARGS s3://\$BUCKET_NAME/$object_key \$TEST_DIR/downloads/${object_key} && stat -c%s \$TEST_DIR/downloads/${object_key}"
        continue
    fi

    if $BINARY cmp "$original_file" "$download_file" >/dev/null 2>&1; then
        success "Downloaded and verified $object_key (size: $download_size bytes, content: identical)"
    else
        error "Data integrity check failed for $object_key" "\$BINARY cmp $SSE_DOWNLOAD_ARGS \$TEST_DIR/$object_key s3://\$BUCKET_NAME/$object_key"
        continue
    fi

    # Single stat call retrieves ETag, Content-Length, and SHA-256 checksum together.
    _stat_json=$($BINARY stat --json "s3://$BUCKET_NAME/$object_key" 2>/dev/null)
    response_etag=$(echo "$_stat_json" | grep '"etag"' | sed 's/.*: *"\(.*\)".*/\1/')
    response_content_length=$(echo "$_stat_json" | grep '"size"' | sed 's/.*: *\([0-9]*\).*/\1/')
    response_checksum=$(echo "$_stat_json" | grep '"sha256"' | sed 's/.*: *"\(.*\)".*/\1/')

    # Check ETag header — S3 Express One Zone uses random/opaque ETags by design;
    # any non-empty ETag is valid (multipart ETags contain "-").
    if [ -n "$response_etag" ]; then
        if [[ "$response_etag" == *"-"* ]]; then
            success "Response ETag (multipart): $response_etag"
        else
            success "Response ETag present: $response_etag"
        fi
    else
        error "Response ETag not found for $object_key" "\$BINARY stat s3://\$BUCKET_NAME/$object_key"
    fi

    # Verify SHA-256 checksum if the server returned one (requires upload with --checksum SHA256).
    # Absence is not a failure — most uploads omit it and integrity is already covered by hsc cmp above.
    if [ -n "$response_checksum" ]; then
        expected_checksum=$(openssl dgst -sha256 -binary "$original_file" | base64)
        if [ "$response_checksum" = "$expected_checksum" ]; then
            success "SHA-256 checksum verified: $response_checksum"
        else
            error "SHA-256 checksum mismatch (expected: $expected_checksum, got: $response_checksum)" "\$BINARY stat s3://\$BUCKET_NAME/$object_key"
        fi
    else
        info "SHA-256 checksum not returned by server for $object_key (skipped)"
    fi

    # Check Content-Length header
    if [ -n "$response_content_length" ]; then
        if [ "$response_content_length" -eq "$original_size" ]; then
            success "Response Content-Length correct: $response_content_length"
        else
            error "Response Content-Length mismatch (expected: $original_size, got: $response_content_length)" "\$BINARY stat s3://\$BUCKET_NAME/$object_key"
        fi
    else
        error "Response Content-Length not found for $object_key" "\$BINARY stat s3://\$BUCKET_NAME/$object_key"
    fi
done

# Step 4: Object functional tests via 'hsc test object'
#
# Replaces the old manual range-request, chunk-boundary, multipart-alignment,
# and EC stripe tests.  Each run:
#   1. Generates a random local file of the given size (using an internal PRNG)
#   2. Uploads it to S3 (multipart when size >= part-size)
#   3. Runs a comprehensive set of range comparisons:
#        - whole object, first/last byte, last 4 bytes
#        - 2-byte straddle and 8-byte crossing at every EC stripe, chunk, and part boundary
#        - 1 KiB and 4 KiB straddles at every chunk and part boundary
#        - complete single-chunk reads, adjacent 2-chunk spans
#        - cross-all-EC-stripe read within each chunk (EC policy only)
#        - large ranges (first half, second half, first 3/4)
#   4. Deletes the object from S3 and the temp local file
#
# Pass/fail counts from each run are merged into the global SUCCESS_COUNT / ERROR_COUNT.
step_time
echo ""
info "Step 4: Object functional tests (hsc test object)..."
info "  Policy: $POLICY  Chunk: $CHUNK_SIZE_STR"
echo ""

# _run_test_object <description> <size> [extra args passed to 'hsc test object']
# Generates a random file of <size> bytes in TEST_DIR, then runs 'hsc test object'
# with -f pointing to that file.  Output is streamed to the terminal in real time
# and per-test pass/fail counts are merged into SUCCESS_COUNT / ERROR_COUNT.
_run_test_object() {
    local desc="$1" size_str="$2"; shift 2
    # Convert size string to bytes for dd / truncate
    local size_bytes
    case "$size_str" in
        *m) size_bytes=$(( ${size_str%m} * 1048576 )) ;;
        *k) size_bytes=$(( ${size_str%k} * 1024 ))    ;;
        *b) size_bytes="${size_str%b}"                  ;;
        *)  size_bytes="$size_str"                      ;;
    esac
    local test_file="$TEST_DIR/hsc_test_${size_str}.dat"
    dd if=/dev/urandom of="$test_file" bs=65536 count=$(( (size_bytes + 65535) / 65536 )) 2>/dev/null
    truncate -s "$size_bytes" "$test_file"

    info "  $desc..."
    local _log rc
    _log=$(mktemp)
    # shellcheck disable=SC2086
    $BINARY test object "$BUCKET_NAME" \
        --chunk-size "$CHUNK_SIZE_STR" \
        --policy "$POLICY" \
        -f "$test_file" \
        "$@" 2>&1 | tee "$_log"
    rc=${PIPESTATUS[0]}
    local passed=0 failed=0 result_line
    result_line=$(grep '^Result:' "$_log")
    rm -f "$_log"
    if [ -n "$result_line" ]; then
        passed=$(echo "$result_line" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+')
        failed=$(echo "$result_line" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+')
        passed=${passed:-0}
        failed=${failed:-0}
        SUCCESS_COUNT=$((SUCCESS_COUNT + passed))
        if [ "${failed:-0}" -gt 0 ]; then
            ERROR_COUNT=$((ERROR_COUNT + failed))
            FAILED_TESTS+=("test object [$desc]: $failed case(s) failed")
            FAILED_CMDS+=("$BINARY test object $BUCKET_NAME --chunk-size $CHUNK_SIZE_STR --policy $POLICY -f $test_file $*")
        fi
    else
        ((ERROR_COUNT++))
        FAILED_TESTS+=("test object [$desc]: command failed (no result produced)")
        FAILED_CMDS+=("$BINARY test object $BUCKET_NAME --chunk-size $CHUNK_SIZE_STR --policy $POLICY -f $test_file $*")
    fi
}

# 1 MiB: fits in one storage chunk; uploaded as a single part.
# Covers within-chunk range reads and basic single-part object integrity.
_run_test_object "1 MiB single-chunk" 1m \
    --part-size 8m

echo ""
# 24 MiB: 3 parts × 8 MiB, 6 storage chunks × 4 MiB.
# Covers all EC stripe (C/4, C/2, 3C/4), chunk, and multipart-part boundaries.
_run_test_object "24 MiB (3×8 MiB parts, 6×4 MiB chunks — all boundary types)" 24m \
    --part-size 8m

echo ""
# Multipart objects with part sizes intentionally misaligned to the storage chunk
# size.  This forces part boundaries to land mid-chunk, exposing server bugs in
# cross-chunk reassembly when parts don't start on chunk edges.
_run_test_object "15 MiB (3×5 MiB parts, misaligned to chunk)" 15m \
    --part-size 5m

echo ""
_run_test_object "18 MiB (3×6 MiB parts, misaligned to chunk)" 18m \
    --part-size 6m

echo ""
_run_test_object "21 MiB (3×7 MiB parts, misaligned to chunk)" 21m \
    --part-size 7m

# Step 5: copyObject — server-side copy at sub-chunk, multipart, and large sizes
step_time
echo ""
info "Step 5: copyObject — server-side copy at various sizes..."
mkdir -p "$TEST_DIR/copy_verify"

# Use the base test files uploaded in Step 2 as copy sources.
# testfile_1m.dat  : sub-chunk (1 MiB < 4 MiB chunk)
# testfile_8m.dat  : multipart, 2 chunks
# testfile_64m.dat : multipart, 16 chunks
COPY_SRCS=("testfile_1m.dat"              "testfile_8m.dat"              "testfile_64m.dat")
COPY_DSTS=("cp_1m.dat"                    "cp_8m.dat"                    "cp_64m.dat")
COPY_ORIGS=("$TEST_DIR/testfile_1m.dat"   "$TEST_DIR/testfile_8m.dat"    "$TEST_DIR/testfile_64m.dat")

for i in "${!COPY_SRCS[@]}"; do
    src=${COPY_SRCS[$i]}; dst=${COPY_DSTS[$i]}; orig=${COPY_ORIGS[$i]}
    info "copyObject $src → $dst..."
    # shellcheck disable=SC2086
    if ! $BINARY cp $SSE_COPY_ARGS "s3://$BUCKET_NAME/$src" "s3://$BUCKET_NAME/$dst" >/dev/null 2>&1; then
        error "copyObject failed: $src → $dst" "\$BINARY cp $SSE_COPY_ARGS s3://\$BUCKET_NAME/$src s3://\$BUCKET_NAME/$dst"
        continue
    fi
    success "copyObject $src → $dst"
    dl="$TEST_DIR/copy_verify/$dst"
    # shellcheck disable=SC2086
    if $BINARY cp $SSE_DOWNLOAD_ARGS "s3://$BUCKET_NAME/$dst" "$dl" >/dev/null 2>&1 \
            && $BINARY cmp "$orig" "$dl" >/dev/null 2>&1; then
        success "copyObject integrity verified: $dst matches $src"
    else
        error "copyObject integrity failed: $dst does not match $src" "\$BINARY cp $SSE_COPY_ARGS s3://\$BUCKET_NAME/$src s3://\$BUCKET_NAME/$dst && \$BINARY cmp $SSE_DOWNLOAD_ARGS $orig s3://\$BUCKET_NAME/$dst"
    fi
done

# Step 6: SSE-C key validation
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
    info "Step 6: SSE-C key validation skipped (set HSC_SSE=sse-c to enable)"
else
info "Step 6: SSE-C key validation tests..."
_SSEC_KEY=$(openssl rand -base64 32)
_SSEC_WRONG=$(openssl rand -base64 32)
_SSEC_SIZE=65536
dd if=/dev/urandom of="$TEST_DIR/$_SSEC_OBJ" bs=$_SSEC_SIZE count=1 2>/dev/null

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

# Step 7: sync --delete and sync --checksum
step_time
echo ""
info "Step 7: sync --delete and sync --checksum tests..."
_SYNC_DIR="$TEST_DIR/sync_test"
_SYNC_PREFIX="sync_test"
mkdir -p "$_SYNC_DIR"

# Create 3 small files for sync tests
dd if=/dev/urandom of="$_SYNC_DIR/sync_a.dat" bs=4096  count=1 2>/dev/null
dd if=/dev/urandom of="$_SYNC_DIR/sync_b.dat" bs=8192  count=1 2>/dev/null
dd if=/dev/urandom of="$_SYNC_DIR/sync_c.dat" bs=16384 count=1 2>/dev/null

# Initial sync: upload all 3 files with --checksum
info "  sync --checksum: uploading 3 files..."
# shellcheck disable=SC2086
if $BINARY sync --checksum=SHA256 $SSE_UPLOAD_ARGS "$_SYNC_DIR/" "s3://$BUCKET_NAME/$_SYNC_PREFIX/" >/dev/null 2>&1; then
    success "sync --checksum: initial sync of 3 files succeeded"
else
    error "sync --checksum: initial sync failed"
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
if $BINARY exists "s3://$BUCKET_NAME/$_SYNC_PREFIX/sync_b.dat" >/dev/null 2>&1; then
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

# Step 8: mv, diff, cat, and ls --versions
step_time
echo ""
info "Step 8: mv / diff / cat / ls --versions..."
mkdir -p "$TEST_DIR/mv_verify" "$TEST_DIR/diff_src" "$TEST_DIR/cat_verify"

# ── mv: rename a small S3 object ──────────────────────────────────────────────
_MV_SRC="testfile_64k.dat"
_MV_DST="mv_renamed_64k.dat"
info "  mv: s3 rename $BUCKET_NAME/$_MV_SRC → $_MV_DST"
if $BINARY mv $SSE_COPY_ARGS \
        "s3://$BUCKET_NAME/$_MV_SRC" "s3://$BUCKET_NAME/$_MV_DST" >/dev/null 2>&1; then
    success "mv: object renamed successfully"
    # source must be gone
    if $BINARY exists "s3://$BUCKET_NAME/$_MV_SRC" >/dev/null 2>&1; then
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
# Ensure diff_src is clean (no leftover files from a previous failed run).
rm -f "$TEST_DIR/diff_src/"*.dat
$BINARY rm --recursive "s3://$BUCKET_NAME/diff_src/" >/dev/null 2>&1 || true
# Populate diff_src with two files, upload them, then diff — expect no differences.
dd if=/dev/urandom of="$TEST_DIR/diff_src/diff_a.dat" bs=4096 count=1 2>/dev/null
dd if=/dev/urandom of="$TEST_DIR/diff_src/diff_b.dat" bs=8192 count=1 2>/dev/null
$BINARY cp $SSE_UPLOAD_ARGS \
    "$TEST_DIR/diff_src/diff_a.dat" "s3://$BUCKET_NAME/diff_src/diff_a.dat" >/dev/null 2>&1
$BINARY cp $SSE_UPLOAD_ARGS \
    "$TEST_DIR/diff_src/diff_b.dat" "s3://$BUCKET_NAME/diff_src/diff_b.dat" >/dev/null 2>&1
info "  diff: comparing local dir to S3 prefix (expect no differences)..."
_diff_out=$($BINARY diff "$TEST_DIR/diff_src/" "s3://$BUCKET_NAME/diff_src/" 2>/dev/null)
if echo "$_diff_out" | grep -q "No differences found"; then
    success "diff: no differences between local dir and S3 prefix"
else
    error "diff: unexpected differences reported: $_diff_out"
fi
# Add an extra local file; diff should report it as only-in-source
dd if=/dev/urandom of="$TEST_DIR/diff_src/diff_extra.dat" bs=1024 count=1 2>/dev/null
_diff_out2=$($BINARY diff "$TEST_DIR/diff_src/" "s3://$BUCKET_NAME/diff_src/" 2>/dev/null)
if echo "$_diff_out2" | grep -q "diff_extra"; then
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
$BINARY rm "s3://$BUCKET_NAME/$_MV_DST"                   >/dev/null 2>&1 || true
$BINARY rm --recursive "s3://$BUCKET_NAME/diff_src/"       >/dev/null 2>&1 || true

# Step 9: new commands (exists / hash / cmp / parts) + --json output
step_time
echo ""
info "Step 9: new commands — exists / hash / cmp / parts + --json output..."

# ── exists ────────────────────────────────────────────────────────────────────
info "  exists: S3 object, bucket, and local-path presence checks..."

_out=$($BINARY exists "s3://$BUCKET_NAME/testfile_1m.dat" 2>/dev/null || true)
if [ "$_out" = "true" ]; then
    success "exists: S3 object testfile_1m.dat reports true"
else
    error "exists: S3 object testfile_1m.dat should exist (got: '$_out')"
fi

_out=$($BINARY exists "s3://$BUCKET_NAME" 2>/dev/null || true)
if [ "$_out" = "true" ]; then
    success "exists: S3 bucket $BUCKET_NAME reports true"
else
    error "exists: S3 bucket $BUCKET_NAME should exist (got: '$_out')"
fi

_out=$($BINARY exists "s3://$BUCKET_NAME/__nonexistent_object__.dat" 2>/dev/null || true)
if [ "$_out" = "false" ]; then
    success "exists: nonexistent S3 object correctly reports false"
else
    error "exists: nonexistent S3 object should report false (got: '$_out')"
fi

_out=$($BINARY exists "$TEST_DIR/testfile_1m.dat" 2>/dev/null || true)
if [ "$_out" = "true" ]; then
    success "exists: local testfile_1m.dat reports true"
else
    error "exists: local testfile_1m.dat should exist (got: '$_out')"
fi

_json=$($BINARY exists --json "s3://$BUCKET_NAME/testfile_1m.dat" 2>/dev/null || true)
if echo "$_json" | grep -q '"exists": true'; then
    success "exists --json: produced JSON with exists:true"
else
    error "exists --json: expected JSON with exists:true (got: '$_json')"
fi

# ── hash ──────────────────────────────────────────────────────────────────────
info "  hash: local and S3 object digests..."

_local_hash=$($BINARY hash "$TEST_DIR/testfile_1k.dat" 2>/dev/null | awk '{print $2}')
_s3_hash=$($BINARY hash "s3://$BUCKET_NAME/testfile_1k.dat" 2>/dev/null | awk '{print $2}')
if [ -n "$_local_hash" ] && [ "$_local_hash" = "$_s3_hash" ]; then
    success "hash: local and S3 SHA256 match ($_local_hash)"
else
    error "hash: local hash '$_local_hash' != S3 hash '$_s3_hash'"
fi

_md5_out=$($BINARY hash --algorithm MD5 "$TEST_DIR/testfile_1k.dat" 2>/dev/null)
if echo "$_md5_out" | grep -q "^MD5"; then
    success "hash --algorithm MD5: output line starts with MD5"
else
    error "hash --algorithm MD5: unexpected output (got: '$_md5_out')"
fi

_hash_json=$($BINARY hash --json "$TEST_DIR/testfile_1k.dat" 2>/dev/null)
if echo "$_hash_json" | grep -q '"algorithm"' && echo "$_hash_json" | grep -q '"value"'; then
    success "hash --json: produced JSON with algorithm and value fields"
else
    error "hash --json: missing expected JSON fields (got: '$_hash_json')"
fi

# ── cmp ───────────────────────────────────────────────────────────────────────
info "  cmp: local-to-S3 match and mismatch cases..."

if $BINARY cmp "$TEST_DIR/testfile_1k.dat" "s3://$BUCKET_NAME/testfile_1k.dat" >/dev/null 2>&1; then
    success "cmp: local testfile_1k.dat and S3 object match"
else
    error "cmp: local testfile_1k.dat and S3 object should match"
fi

if ! $BINARY cmp "s3://$BUCKET_NAME/testfile_1k.dat" "s3://$BUCKET_NAME/testfile_1m.dat" >/dev/null 2>&1; then
    success "cmp: correctly detected mismatch between 1k and 1m objects"
else
    error "cmp: should have reported mismatch between 1k and 1m objects"
fi

_cmp_json=$($BINARY cmp --json "$TEST_DIR/testfile_1k.dat" "s3://$BUCKET_NAME/testfile_1k.dat" 2>/dev/null || true)
if echo "$_cmp_json" | grep -q '"identical": true'; then
    success "cmp --json: produced JSON with identical:true"
else
    error "cmp --json: expected JSON with identical:true (got: '$_cmp_json')"
fi

# ── parts ─────────────────────────────────────────────────────────────────────
info "  parts: object-part metadata..."

if $BINARY parts "s3://$BUCKET_NAME/testfile_1k.dat" >/dev/null 2>&1; then
    success "parts: metadata for single-put object succeeded"
else
    error "parts: metadata for single-put object failed"
fi

# testfile_64m.dat (64 MiB) was uploaded via multipart in Step 2
if $BINARY parts "s3://$BUCKET_NAME/testfile_64m.dat" >/dev/null 2>&1; then
    success "parts: metadata for multipart object succeeded"
else
    error "parts: metadata for multipart object failed"
fi

_parts_json=$($BINARY parts --json "s3://$BUCKET_NAME/testfile_64m.dat" 2>/dev/null)
if echo "$_parts_json" | grep -q '"parts"'; then
    success "parts --json: produced JSON with parts field"
else
    error "parts --json: expected JSON with parts field (got first 80 chars: '${_parts_json:0:80}')"
fi

if ! $BINARY parts "s3://$BUCKET_NAME" >/dev/null 2>&1; then
    success "parts: correctly rejected bucket-only path (no key)"
else
    error "parts: should have failed on bucket-only path"
fi

# ── --json output smoke tests for existing commands ───────────────────────────
info "  --json output: ls, stat, cmp, diff..."

_ls_json=$($BINARY ls --json "s3://$BUCKET_NAME/" 2>/dev/null)
if echo "$_ls_json" | grep -q '"key"'; then
    success "ls --json: produced JSON with key field"
else
    error "ls --json: expected JSON with key field (got first 80 chars: '${_ls_json:0:80}')"
fi

_stat_json=$($BINARY stat --json "s3://$BUCKET_NAME/testfile_1k.dat" 2>/dev/null)
if echo "$_stat_json" | grep -q '"size"'; then
    success "stat --json: produced JSON with size field"
else
    error "stat --json: expected JSON with size field (got: '$_stat_json')"
fi

# shellcheck disable=SC2086
_cmp_json=$($BINARY cmp --json $SSE_DOWNLOAD_ARGS "$TEST_DIR/testfile_1k.dat" "s3://$BUCKET_NAME/testfile_1k.dat" 2>/dev/null || true)
if echo "$_cmp_json" | grep -q '"identical"'; then
    success "cmp --json: produced JSON with identical field"
else
    error "cmp --json: expected JSON with identical field (got: '$_cmp_json')"
fi

_diff_json_dir=$(mktemp -d)
dd if=/dev/urandom of="$_diff_json_dir/tmp_diff_json.dat" bs=1024 count=1 2>/dev/null
# shellcheck disable=SC2086
$BINARY cp $SSE_UPLOAD_ARGS "$_diff_json_dir/tmp_diff_json.dat" \
    "s3://$BUCKET_NAME/diff_json_test/tmp_diff_json.dat" >/dev/null 2>&1 || true
_diff_json=$($BINARY diff --json "$_diff_json_dir/" "s3://$BUCKET_NAME/diff_json_test/" 2>/dev/null)
if echo "$_diff_json" | grep -q '"identical"'; then
    success "diff --json: produced JSON with identical field"
else
    error "diff --json: expected JSON with identical field (got: '$_diff_json')"
fi
$BINARY rm "s3://$BUCKET_NAME/diff_json_test/tmp_diff_json.dat" >/dev/null 2>&1 || true
rm -rf "$_diff_json_dir"

# Step 10: Delete all objects
step_time
echo ""
if [ $ERROR_COUNT -gt 0 ]; then
    info "Step 10: Skipping object deletion — $ERROR_COUNT test(s) failed"
    info "  S3 objects preserved at:    s3://$BUCKET_NAME/"
    info "  Local test files at:        $TEST_DIR"
    info "  Rerun commands will be written to: ./rerun_failed.sh"
else
    info "Step 10: Deleting all objects..."
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
fi

# Step 11: Delete bucket
step_time
echo ""
if [[ -n "$BUCKET_PROVIDED" ]]; then
    info "Step 11: Skipping bucket deletion (bucket '$BUCKET_NAME' was provided by caller)"
elif [ $ERROR_COUNT -gt 0 ]; then
    info "Step 11: Skipping bucket deletion — preserving for rerun"
else
    info "Step 11: Deleting bucket '$BUCKET_NAME'..."
    if $BINARY rb "s3://$BUCKET_NAME"; then
        success "Bucket deleted successfully"
    else
        error "Failed to delete bucket"
    fi
fi

# Cleanup local test files
echo ""
if [ $ERROR_COUNT -gt 0 ]; then
    info "Preserving local test files in $TEST_DIR for rerun"
else
    info "Cleaning up local test files..."
    rm -rf "$TEST_DIR"
    success "Cleanup complete"
fi

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
        echo "# Edit BUCKET_NAME / BINARY / TEST_DIR below, then: bash rerun_failed.sh"
        echo "BUCKET_NAME=\"\${BUCKET_NAME:-$BUCKET_NAME}\""
        echo "BINARY=\"\${BINARY:-$BINARY}\""
        echo "TEST_DIR=\"\${TEST_DIR:-$TEST_DIR}\"  # local test files preserved from failed run"
        echo ""
        echo "set +e"
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

# Exit with non-zero status if any tests failed so callers (CI, run_test.sh,
# tcp_tests.sh) can detect failure via the process exit code.
[ $ERROR_COUNT -eq 0 ]

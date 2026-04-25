# Changelog

All notable changes to hsc will be documented in this file.

## [Unreleased]

### Added
- `test object` command: upload a local file (or randomly-generated data) to S3 and verify it
  with systematic byte-range comparisons covering multipart part boundaries, server chunk
  boundaries, and EC stripe boundaries (C/4 and C/2 within each chunk)
  - `-f <path>`: use an existing local file; `-b <size>`: generate random test data (e.g. `6m`)
  - `--chunk-size`: server storage chunk size (default: `4m`)
  - `--part-size`: multipart upload part size/threshold (default: `8m`)
  - `--keep`: retain the S3 object after the test; default is to delete on completion
  - `--json`: emit a structured JSON summary including per-test pass/fail status
  - Exits `1` if any comparison fails

## [0.3.2] - 2026-04-03

### Added
- `exists` command: test whether a local path, S3 bucket, or S3 object exists
  - Exits `0` when the target exists, `1` when it does not
  - `--json` emits `{"path": "...", "exists": true/false}`
- `hash` command: compute a digest for a local file or S3 object by streaming its content
  - Supports `MD5`, `CRC32`, `CRC32C`, `SHA1`, `SHA256` via `--algorithm` (default: `SHA256`)
  - `--json` emits `{"path", "algorithm", "value", "size"}`
- `parts` command: show multipart upload metadata for an S3 object
  - Default path uses `HeadObject` (works on any S3-compatible server); reports part number and size
  - `--attributes` switches to `GetObjectAttributes` (AWS-native; adds per-part checksums)
  - `--json` emits structured JSON

### Changed
- `cmp` command: merged the former `verify` command; `cmp` now covers all comparison scenarios
  - `--algorithm` selects the hash algorithm printed on a successful match (default: `SHA256`)
  - `--json` emits a structured result object
  - Full-file hash is skipped when `--range`/`--offset`/`--size` is set (partial comparison)

### Fixed
- **`cmp` `bytes_compared` off-by-one**: when a byte mismatch was found at position `i` in the
  current read chunk, the reported `bytes_compared` field omitted the `i` bytes successfully
  compared within that chunk; now correctly reports `prior_bytes + i`
- **RDMA deprecation**: replaced deprecated `process_reply_token` with `process_rdma_reply`;
  parses the `x-amz-rdma-reply` header as a numeric HTTP status code and reads `x-amz-rdma-bytes`
  for the transferred byte count
- **`examples/s3_functional_test.sh` exit code**: script previously exited `0` regardless of
  failures; now exits `1` when any test fails
- **Chunk-midpoint range coverage**: added `chunk1_half` (C + C/2) and `chunk2_half` (2C + C/2)
  objects to chunk-boundary tests, covering byte ranges that cross a chunk boundary mid-object
  (previously only `±1` boundary objects were tested)
- **Random test file content**: chunk-boundary and multipart test files are now filled with
  random data via `/dev/urandom` instead of `truncate` sparse zeros; sparse files cannot detect
  bugs where a server returns bytes from the wrong offset within a zero-filled chunk

### Build
- Updated cuobj RDMA provider path from `providers/cuobj` to `providers/cuobj-client` following
  upstream rename; updated `.so` name to `libs3_rdma_cuobj_client.so` in Makefile and package
  build scripts

## [0.3.1] - 2026-03-31

### Added
- `--disable-multipart` option for `cp`, `sync`, and `mv`: forces a single PUT
  for all uploads regardless of file size (max 5 GiB per object)
- `--part-size <SIZE>` option for `cp`, `sync`, and `mv`: sets both the multipart
  threshold and chunk size per command (e.g. `16m`, `256m`, `1g`, or plain bytes);
  conflicts with `--disable-multipart`
  - Priority: CLI flag > `~/.aws/config` `[s3]` `multipart_threshold`/`multipart_chunksize` > 8 MiB default
- `--sse-c` / `--sse-c-key` options for `cmp`: enables SSE-C decryption when
  comparing S3 objects encrypted with customer-provided keys

### Fixed
- **Multipart upload checksums** (`cp --checksum`, `sync --checksum`): checksums
  now work correctly on Cloudian and other S3-compatible servers
  - `UploadPart` requests send the checksum as a plain request header; previously
    the SDK used `aws-chunked` trailing encoding which most S3-compatible servers
    reject
  - `CompleteMultipartUpload` uses a locally computed fallback checksum when the
    server does not echo the per-part checksum back in the `UploadPart` response
    (affects RDMA paths where the HTTP body is empty)
  - `RequestChecksumCalculation::WhenRequired` prevents the SDK from
    auto-injecting `CRC32C` into every multipart upload; checksums are only sent
    when explicitly requested via `--checksum`
- **Custom headers on multipart uploads** (`-H` / `--custom-header`):
  `x-amz-meta-*`, `x-amz-acl`, `x-amz-grant-*`, and `x-amz-tagging` are now
  injected only into `PutObject` and `CreateMultipartUpload`, not into
  `UploadPart` or `CompleteMultipartUpload` (which reject them with "Metadata
  cannot be specified in this context")
- **`cmp` range performance**: skip the `HEAD` request for S3 objects when a byte
  range or size limit is already given; halves S3 request count for parallel
  range-integrity checks
- **Retry limit**: default `max_attempts` raised from 3 to 10 to absorb transient
  throttling under high-concurrency workloads; override with `AWS_MAX_ATTEMPTS`
- **`examples/s3_functional_test.sh`** reliability improvements:
  - Test objects and local files are preserved on failure so failed tests can be
    rerun individually via the generated `rerun_failed.sh` script
  - All RERUN commands corrected — reference `$TEST_DIR` paths instead of
    attempting to recreate files in `/tmp`
  - Test data generated with `truncate` (O(1)) instead of `dd if=/dev/random`,
    which could produce undersized files due to partial `read()` syscalls
  - Remaining `dd` calls use `iflag=fullblock` to prevent partial reads
  - Leftover objects from previous runs are deleted at the start of each run

### Internal
- Extracted `upload_part_http()` helper in `cp.rs`, eliminating three
  near-identical `UploadPart` build-and-send blocks (~90 lines → ~20 lines)

## [0.3.0] - 2026-03-29

### Added
- `ls --versions` flag: list all object versions and delete markers for a bucket or key prefix
  - Paginates automatically through large version histories
  - Output columns: KEY, VERSION-ID, LATEST, TYPE, LAST-MODIFIED, SIZE
  - `--human-readable` flag formats sizes as KB/MB/GB
- `sync --checksum`: thread checksum mode/algorithm through to upload and
  download calls, matching the existing `cp --checksum` behaviour
  (accepted values: `ENABLED`, `CRC32`, `CRC32C`, `SHA1`, `SHA256`; bare flag = `ENABLED`)
- `sync --delete`: remove destination entries not present in source
  - local→S3: deletes S3 objects absent from the local source tree
  - S3→local: deletes local files absent from the S3 source prefix
  - S3→S3: deletes destination objects absent from the source prefix
- SSE (Server-Side Encryption) options for `cp`, `sync`, and `mv`:
  - `--sse AES256|aws:kms|aws:kms:dsse` — server-managed encryption
  - `--sse-kms-key-id <ARN>` — KMS key ARN or alias (used with `aws:kms`)
  - `--sse-c AES256` — customer-provided key algorithm
  - `--sse-c-key <base64-key>` — 256-bit customer key, base64-encoded (MD5 computed automatically)
  - `--sse-c-copy-source AES256` — SSE-C algorithm for the copy source (S3→S3 only)
  - `--sse-c-copy-source-key <base64-key>` — customer key for source decryption (S3→S3 only)
- `cmp` command: byte-by-byte comparison of two local files, S3 objects, or a mix
  - Exits 0 if identical, 1 if they differ
  - Reports first differing byte (1-based) and line number to stderr
  - Supports `--range`, `--offset`, and `--size` (same semantics as `cat`)
  - Works with any combination of local paths and `s3://` URIs
- `tests/test_cmp.sh`: dedicated test script for `cmp` (14 tests covering local, range, and S3)
- `examples/s3_functional_test.sh` improvements:
  - `HSC_SSE` env var (`AES256`, `aws:kms`, `sse-c`) to run all tests with encryption
  - `HSC_SSE_KMS_KEY_ID` and `HSC_SSE_C_KEY` companion variables
  - Step 8e: SSE-C key validation — correct key succeeds; missing or wrong key rejected
  - Step 8f: `sync --delete` and `sync --checksum` functional tests
  - Step 8g: `mv`, `diff`, `cat`, and `ls --versions` functional tests
  - Optional bucket argument — skips `mb`/`rb` when a pre-existing bucket is supplied
  - EC stripe boundary tests (Steps 8a–8d) for 3-replica / EC 2+1 / EC 4+2 storage policies
  - Numbered failed-test summary at end of run with rerun commands written to `rerun_failed.sh`

### Fixed
- Empty bucket check in `examples/s3_functional_test.sh`: replaced `wc -l` with
  `grep -c "^[0-9]"` to avoid false positives from the `ls` summary footer line
- Sync test files now use `/dev/random` for content, consistent with all other test files

## [0.1.0] - 2026-02-25

### Added
- Initial release
- 10 core commands: mb, rb, ls, cp, sync, mv, rm, stat, diff, cat
- Multipart upload support with configurable thresholds
- Checksum validation (CRC32, CRC32C, SHA1, SHA256)
- Glob-based include/exclude filtering
- Recursive directory operations
- Range reads for cat command
- Content-based diff comparison
- Full AWS configuration support
- S3-compatible endpoint support
- Comprehensive test suite

### Features
- **Bucket Operations**: Create, delete, and list buckets
- **Object Operations**: Copy, move, remove, and list objects
- **Smart Sync**: Incremental synchronization based on file size
- **Detailed Stats**: File and object metadata with checksums
- **Directory Diff**: Compare local and S3 locations
- **Streaming Cat**: Output file content with range support
- **AWS Integration**: Respects AWS credentials, profiles, and config
- **Performance**: Async I/O with Tokio, streaming transfers

### Configuration
- AWS config file support for multipart settings
- Environment variable support (AWS_*)
- Profile-based configuration
- Custom endpoint URLs for S3-compatible services

### Documentation
- Complete README with quick start and examples
- Command reference guide
- Environment variable documentation
- Usage examples
- Test scripts for all features

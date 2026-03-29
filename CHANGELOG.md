# Changelog

All notable changes to hsc will be documented in this file.

## [Unreleased]

### Added
- `versions` command: list all object versions and delete markers for a bucket or key prefix
  - Paginates automatically through large version histories
  - Output columns: KEY, VERSION-ID, LATEST, TYPE, LAST-MODIFIED, SIZE
  - `--human-readable` flag formats sizes as KB/MB/GB
- `--checksum` option for `sync`: thread checksum mode/algorithm through to upload and
  download calls, matching the existing `cp --checksum` behaviour
  (accepted values: `ENABLED`, `CRC32`, `CRC32C`, `SHA1`, `SHA256`; bare flag = `ENABLED`)
- `--delete` option for `sync`: remove destination entries not present in source
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
  - Step 8e: SSE-C key validation — verifies correct key succeeds, missing/wrong key rejected
  - Step 8f: `sync --delete` and `sync --checksum` functional tests
  - Optional bucket argument — skips `mb`/`rb` when a pre-existing bucket is supplied
  - EC stripe boundary tests (Steps 8a–8d) for 3-replica / EC 2+1 / EC 4+2 storage policies

### Fixed
- Empty bucket check in `examples/s3_functional_test.sh`: replaced `wc -l` with
  `grep -c "^[0-9]"` to avoid false positives from the `ls` summary footer line

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

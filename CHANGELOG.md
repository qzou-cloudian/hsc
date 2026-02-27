# Changelog

All notable changes to hsc will be documented in this file.

## [Unreleased]

### Added
- `cmp` command: byte-by-byte comparison of two local files, S3 objects, or a mix
  - Exits 0 if identical, 1 if they differ
  - Reports first differing byte (1-based) and line number to stderr
  - Supports `--range`, `--offset`, and `--size` (same semantics as `cat`)
  - Works with any combination of local paths and `s3://` URIs
- `tests/test_cmp.sh`: dedicated test script for `cmp` (14 tests covering local, range, and S3)

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

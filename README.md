# hsc - High-Performance S3 CLI

A fast, feature-rich command-line tool for AWS S3 and S3-compatible storage, written in Rust.


## Features

- **11 Essential Commands**: `mb`, `rb`, `ls`, `cp`, `sync`, `mv`, `rm`, `stat`, `diff`, `cat`, `cmp`
- **Multipart Upload**: Automatic multipart transfers for large files with configurable thresholds
- **Checksum Validation**: Support for CRC32, CRC32C, SHA1, and SHA256
- **Smart Filtering**: Include/exclude patterns using glob syntax
- **S3-Compatible**: Works with AWS S3, MinIO, Cloudian, and other S3-compatible services
- **Full AWS Config**: Respects AWS credentials, config files, and environment variables

## Installation

```bash
cargo install hsc
```

Or build from source:

```bash
git clone <repository-url>
cd hsc
cargo build --release
```

### Verify files match

```bash
# Verify local and S3 copies are identical
hsc cmp ./myfile.txt s3://my-bucket/myfile.txt

# Verify a specific byte range
hsc cmp --range 0-999 ./header.bin s3://bucket/header.bin
```

## Quick Start

```bash
# List buckets
hsc ls

# Create bucket
hsc mb s3://my-bucket

# Upload file
hsc cp myfile.txt s3://my-bucket/

# Sync directory
hsc sync ./local-dir s3://my-bucket/prefix/

# Get file info
hsc stat s3://my-bucket/myfile.txt

# Compare directories
hsc diff ./local-dir s3://my-bucket/prefix/

# Download with range
hsc cat s3://my-bucket/file.txt --range 0-100
```

## Commands

### Bucket Operations

- **`mb s3://bucket`** - Create a new bucket
- **`rb s3://bucket [--force]`** - Remove bucket (use --force to delete all objects)
- **`ls [s3://bucket[/prefix]] [--recursive]`** - List buckets or objects

### Object Operations

- **`cp <source> <dest> [--recursive]`** - Copy files/objects
- **`mv <source> <dest> [--recursive]`** - Move files/objects
- **`rm <path> [--recursive]`** - Remove objects
- **`sync <source> <dest>`** - Synchronize directories (copies only changed files)

### Information Commands

- **`stat <path> [--recursive]`** - Display detailed file/object metadata
- **`diff <source> <dest>`** - Compare directories or buckets
- **`cat <path> [--range <start-end>]`** - Output file content to stdout
- **`cmp <path1> <path2> [--range <start-end>]`** - Compare two files or objects byte-by-byte

## Configuration

### AWS Credentials

hsc uses standard AWS configuration:

```ini
# ~/.aws/credentials
[default]
aws_access_key_id = YOUR_KEY
aws_secret_access_key = YOUR_SECRET

# ~/.aws/config
[default]
region = us-east-1
```

### Multipart Upload Settings

Configure automatic multipart uploads in `~/.aws/config`:

```ini
[s3]
multipart_threshold = 10MB
multipart_chunksize = 5MB
```

Supported formats: Plain bytes, MB, M, KB, K, GB, G (default: 8MB)

### Global Options

```bash
--profile <name>        # AWS profile to use
--region <region>       # AWS region
--endpoint-url <url>    # Custom S3 endpoint
--no-verify-ssl         # Disable SSL verification
--debug                 # Enable debug output
--version               # Show version
```

### Environment Variables

- `AWS_PROFILE` - AWS profile name
- `AWS_REGION` - AWS region
- `AWS_ACCESS_KEY_ID` - Access key
- `AWS_SECRET_ACCESS_KEY` - Secret key
- `AWS_SESSION_TOKEN` - Session token
- `AWS_ENDPOINT_URL` - Custom endpoint URL
- `AWS_CONFIG_FILE` - Config file location
- `AWS_SHARED_CREDENTIALS_FILE` - Credentials file location

## Advanced Features

### Filtering

Use glob patterns to filter files:

```bash
# Copy only .txt files
hsc cp --include "*.txt" ./dir s3://bucket/

# Copy all except .log files
hsc cp --exclude "*.log" ./dir s3://bucket/

# Multiple patterns
hsc sync --include "*.jpg" --include "*.png" ./photos s3://bucket/
```

### Checksums

Validate data integrity with checksums:

```bash
# Calculate checksums for local files
hsc stat myfile.txt --checksum-mode ENABLED --checksum-algorithm SHA256

# Verify S3 object checksums
hsc cp file.txt s3://bucket/ --checksum-algorithm CRC32C
```

### Range Reads

Read specific byte ranges:

```bash
# Read first 1000 bytes
hsc cat s3://bucket/file.txt --range 0-999

# Read from offset
hsc cat file.txt --offset 1000 --size 500

# Pipe to other tools
hsc cat s3://bucket/log.txt --range 0-1000 | grep ERROR
```

### Content Comparison

Compare directories by size and content:

```bash
# Compare by size (default)
hsc diff ./local-dir s3://bucket/prefix/

# Compare by content (ETag/MD5)
hsc diff --compare-content ./dir1 ./dir2
```

## S3-Compatible Services

Works with MinIO, Cloudian, and other S3-compatible storage:

```bash
# Use environment variable
export AWS_ENDPOINT_URL=https://s3.example.com
hsc ls

# Or use command-line option
hsc --endpoint-url https://s3.example.com ls
```

## Examples

### Backup local directory to S3

```bash
hsc sync --exclude "*.tmp" --exclude ".git/*" ./myproject s3://backups/myproject/
```

### Download large file with verification

```bash
hsc cp s3://bucket/large-file.zip ./ --checksum-mode ENABLED
```

### Mirror S3 bucket

```bash
hsc sync s3://source-bucket/ s3://dest-bucket/
```

### Find differences between environments

```bash
hsc diff s3://prod-bucket/data/ s3://staging-bucket/data/ --compare-content
```

### Monitor log files

```bash
# Get last 1000 bytes
hsc cat s3://logs/app.log --offset $(hsc stat s3://logs/app.log | grep Size | awk '{print $3-1000}') | tail
```

## Performance

- **Async I/O**: Built on Tokio for high concurrency
- **Streaming**: Memory-efficient streaming for large files
- **Multipart**: Automatic multipart uploads for files over threshold
- **Smart Sync**: Only transfers changed files

## Testing

```bash
# Run all tests
cd tests
./test_s3.sh

# Test specific features
./test_multipart.sh
./test_stat_comprehensive.sh
./test_diff.sh
./test_cmp.sh
```

See `examples/` directory for AWS configuration samples.

## License

Licensed under MIT License ([LICENSE](LICENSE))

## Acknowledgements

- This tool was inspired by AWS S3 CLI and MinIO client.
- This tool is AI-generated code using Github Copilot CLI with `claude-sonnet-4.5` model.

## Contributing

Contributions welcome! Please feel free to submit issues or pull requests.

## See Also

- [AWS CLI S3 Commands](https://docs.aws.amazon.com/cli/latest/reference/s3/)
- [MinIO Client](https://min.io/docs/minio/linux/reference/minio-mc.html)
- [AWS SDK for Rust](https://github.com/awslabs/aws-sdk-rust)

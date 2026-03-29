# hsc - High-Performance S3 CLI

A fast, feature-rich command-line tool for AWS S3 and S3-compatible storage, written in Rust.
hsc supports **RDMA-accelerated transfers** via the NVIDIA cuObject API, enabling direct data transfers
between GPU memory or system memory and S3 compatible object storage using RDMA.


## Features

- **12 Essential Commands**: `mb`, `rb`, `ls`, `cp`, `sync`, `mv`, `rm`, `stat`, `diff`, `cat`, `cmp`
- **RDMA Transfers**: GPU-direct data paths via NVIDIA cuObject — zero-copy between object storage and GPU memory or system memory
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

- **`mb s3://bucket [--ignore-existing]`** - Create a new bucket
- **`rb s3://bucket [--force]`** - Remove bucket (use --force to delete all objects)
- **`ls [s3://bucket[/prefix]] [--recursive] [--versions] [--human-readable]`** - List buckets or objects; `--versions` lists all object versions and delete markers

### Object Operations

- **`cp <source> <dest> [--recursive] [--checksum [ALGO]]`** - Copy files/objects
- **`mv <source> <dest> [--recursive]`** - Move files/objects
- **`rm <path> [--recursive]`** - Remove objects
- **`sync <source> <dest> [--checksum [ALGO]] [--delete]`** - Synchronize directories (copies only changed files; `--delete` removes destination entries absent from source)

### Information Commands

- **`stat <path> [--recursive]`** - Display detailed file/object metadata
- **`diff <source> <dest>`** - Compare directories or buckets
- **`cat <path> [--range <start-end>]`** - Output file content to stdout
- **`cmp <path1> <path2> [--range <start-end>]`** - Compare two files or objects byte-by-byte
- **`versions s3://bucket[/prefix] [--human-readable]`** - List all object versions and delete markers

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
--profile <name>                 # AWS profile to use
--region <region>                # AWS region
--endpoint-url <url>             # Custom S3 endpoint
--no-verify-ssl                  # Disable SSL verification
--debug                          # Enable debug output
--cli-connect-timeout <secs>     # TCP connect timeout (0 = no timeout)
--cli-read-timeout <secs>        # Socket read timeout (0 = no timeout)
-H, --custom-header <KEY:VALUE>  # Add a custom HTTP header to every request (repeatable)
--no-sign-request                # Send unsigned requests (for public buckets)
--rdma [PROVIDER]                # Enable RDMA transfers (requires rdma feature build)
--version                        # Show version
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
- `HSC_RDMA` - RDMA provider: `auto`, `cuobj`, `mock`, `true`/`1` (enable), `false`/`0` (disable)

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
# Verify S3 object checksums on upload
hsc cp file.txt s3://bucket/ --checksum CRC32C

# Calculate checksums for local files
hsc stat myfile.txt --checksum SHA256

# Sync with checksum verification
hsc sync --checksum CRC32 ./local-dir s3://bucket/prefix/
```

### Sync with Delete

Mirror a source exactly, removing destination entries not present in the source:

```bash
# Remove S3 objects that no longer exist locally
hsc sync --delete ./local-dir s3://bucket/prefix/

# Mirror one S3 prefix to another
hsc sync --delete s3://source-bucket/data/ s3://dest-bucket/data/

# Combine with --checksum for full verification
hsc sync --delete --checksum SHA256 ./local-dir s3://bucket/prefix/
```

### Server-Side Encryption

Encrypt objects at rest using S3-managed keys, AWS KMS, or customer-provided keys:

```bash
# S3-managed AES-256 encryption
hsc cp file.txt s3://bucket/ --sse AES256

# AWS KMS encryption
hsc cp file.txt s3://bucket/ --sse aws:kms
hsc cp file.txt s3://bucket/ --sse aws:kms --sse-kms-key-id arn:aws:kms:us-east-1:123:key/abc

# Customer-provided key (SSE-C) — you manage the 256-bit key
KEY=$(openssl rand -base64 32)
hsc cp file.txt s3://bucket/ --sse-c AES256 --sse-c-key "$KEY"
hsc cp s3://bucket/file.txt ./ --sse-c AES256 --sse-c-key "$KEY"

# SSE-C for S3-to-S3 copy (source and destination may use different keys)
hsc cp s3://bucket/src.txt s3://bucket/dst.txt \
    --sse-c-copy-source AES256 --sse-c-copy-source-key "$SRC_KEY" \
    --sse-c AES256 --sse-c-key "$DST_KEY"

# All encryption options work with sync and mv too
hsc sync --sse AES256 ./local-dir s3://bucket/prefix/
```

### Object Versions

List all versions and delete markers for versioned buckets:

```bash
# List all versions in a bucket
hsc ls --versions s3://my-bucket

# List versions for a specific key prefix
hsc ls --versions s3://my-bucket/path/to/

# Human-readable sizes
hsc ls --versions --human-readable s3://my-bucket/prefix/
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

Compare files/objects with specific byte ranges:

```bash
# Verify local and S3 copies are identical
hsc cmp ./myfile.txt s3://my-bucket/myfile.txt

# Verify a specific byte range
hsc cmp --range 0-999 ./header.bin s3://bucket/header.bin
```

### RDMA Transfers

RDMA support accelerates large object transfers by allowing the S3 server to
read/write directly into registered host (or GPU) memory, bypassing the CPU.

Build with RDMA support:

```bash
# Mock provider (no hardware required, useful for testing)
cargo build --release --features rdma

# NVIDIA cuObject provider (requires cuObject SDK and libhsc_rdma_cuobj.so at runtime)
cargo build --release --features cuobj
```

Enable RDMA at runtime:

```bash
# Auto-select best available provider
hsc --rdma cp large-file.bin s3://bucket/

# Use mock provider (for testing)
hsc --rdma mock cp s3://bucket/file.bin ./

# Via environment variable (applies to all commands)
export HSC_RDMA=auto
hsc cp large-file.bin s3://bucket/
```

Configure via `~/.aws/config`:

```ini
[default]
rdma = auto       # enable RDMA, auto-select provider
# rdma = cuobj # prefer cuObject provider
# rdma = mock     # always use mock provider
# rdma = false    # disable
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

### Public Buckets (No Signing)

Access publicly readable buckets without AWS credentials:

```bash
hsc --no-sign-request ls s3://my-public-bucket
hsc --no-sign-request cp s3://my-public-bucket/file.txt ./
```

### Custom Headers

Inject arbitrary HTTP headers into every request (useful for proxies or custom auth):

```bash
hsc -H "x-forwarded-for:10.0.0.1" ls
hsc -H "x-request-id:abc123" -H "x-tenant:acme" cp file.txt s3://bucket/
```

### Timeouts

```bash
# 5-second connect timeout, 30-second read timeout
hsc --cli-connect-timeout 5 --cli-read-timeout 30 cp large.bin s3://bucket/
```

## Examples

### Backup local directory to S3

```bash
hsc sync --exclude "*.tmp" --exclude ".git/*" ./myproject s3://backups/myproject/
```

### Download large file with verification

```bash
hsc cp s3://bucket/large-file.zip ./ --checksum
```

### Mirror S3 bucket

```bash
hsc sync --delete s3://source-bucket/ s3://dest-bucket/
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

### More Examples

- **Quick reference:** [QUICK_REFERENCE.md](QUICK_REFERENCE.md)
- **Full reference:** [docs/REFERENCE.md](docs/REFERENCE.md)

See `examples/` directory for real-world usage:
- **[S3 Functional Test Script](examples/s3_functional_test.sh)** - Tests bucket operations, object put/get with various sizes, and range requests.

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
- [NVIDIA cuObject: GPUDirect Storage for Objects](https://docs.nvidia.com/gpudirect-storage/cuobject/)

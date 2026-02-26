# Command Reference

Quick reference for all hsc commands with detailed options.

## Global Options

Available for all commands:

```
--profile <name>        AWS profile to use
--region <region>       AWS region override
--endpoint-url <url>    Custom S3 endpoint (for S3-compatible services)
--no-verify-ssl         Disable SSL certificate verification
--debug                 Enable debug logging
--version               Show version information
```

## Commands

### mb - Make Bucket

Create a new S3 bucket.

```bash
hsc mb s3://bucket-name
```

**Options:** None

**Examples:**
```bash
hsc mb s3://my-new-bucket
hsc --region eu-west-1 mb s3://eu-bucket
```

### rb - Remove Bucket

Remove an S3 bucket.

```bash
hsc rb s3://bucket-name [--force]
```

**Options:**
- `--force` - Delete all objects in bucket before removing

**Examples:**
```bash
hsc rb s3://empty-bucket
hsc rb s3://bucket-with-files --force
```

### ls - List

List buckets or objects.

```bash
hsc ls [s3://bucket[/prefix]] [--recursive]
```

**Options:**
- `--recursive` - List all objects recursively

**Examples:**
```bash
hsc ls                              # List all buckets
hsc ls s3://bucket                  # List objects in bucket
hsc ls s3://bucket/prefix/          # List objects with prefix
hsc ls s3://bucket/ --recursive     # List all objects recursively
```

### cp - Copy

Copy files or objects between local filesystem and S3.

```bash
hsc cp <source> <dest> [options]
```

**Options:**
- `--recursive` - Copy directories recursively
- `--include <pattern>` - Include only files matching pattern (can be repeated)
- `--exclude <pattern>` - Exclude files matching pattern (can be repeated)
- `--checksum-mode <mode>` - ENABLED or DISABLED (for single file)
- `--checksum-algorithm <alg>` - CRC32, CRC32C, SHA1, or SHA256

**Examples:**
```bash
hsc cp file.txt s3://bucket/                    # Upload file
hsc cp s3://bucket/file.txt ./                  # Download file
hsc cp --recursive ./dir s3://bucket/prefix/    # Upload directory
hsc cp --include "*.jpg" ./photos s3://bucket/  # Upload only .jpg files
hsc cp file.txt s3://bucket/ --checksum-algorithm SHA256
```

### mv - Move

Move files or objects (copy then delete source).

```bash
hsc mv <source> <dest> [options]
```

**Options:**
- `--recursive` - Move directories recursively
- `--include <pattern>` - Include only files matching pattern
- `--exclude <pattern>` - Exclude files matching pattern

**Examples:**
```bash
hsc mv file.txt s3://bucket/newname.txt
hsc mv s3://bucket/old/ s3://bucket/new/ --recursive
```

### rm - Remove

Remove objects from S3.

```bash
hsc rm <path> [options]
```

**Options:**
- `--recursive` - Remove all objects with prefix
- `--include <pattern>` - Remove only files matching pattern
- `--exclude <pattern>` - Exclude files from removal

**Examples:**
```bash
hsc rm s3://bucket/file.txt
hsc rm s3://bucket/prefix/ --recursive
hsc rm s3://bucket/logs/ --recursive --include "*.log"
```

### sync - Synchronize

Synchronize directories (only copy new/changed files).

```bash
hsc sync <source> <dest> [options]
```

**Options:**
- `--include <pattern>` - Include only files matching pattern
- `--exclude <pattern>` - Exclude files matching pattern

**Behavior:**
- Compares file sizes
- Only uploads/downloads files that are new or have changed
- More efficient than `cp` for incremental backups

**Examples:**
```bash
hsc sync ./local-dir s3://bucket/backup/        # Backup local to S3
hsc sync s3://bucket/data/ ./local-cache/       # Download updates
hsc sync --exclude "*.tmp" ./project s3://backup/
```

### stat - Statistics

Display detailed information about files, directories, objects, or buckets.

```bash
hsc stat <path> [options]
```

**Options:**
- `--recursive` - Process directories/prefixes recursively
- `--checksum-mode <mode>` - ENABLED or DISABLED (for local files)
- `--checksum-algorithm <alg>` - CRC32, CRC32C, SHA1, or SHA256 (for local files)

**Local File Output:**
- Name, Type, Size
- Modified time, Access time, Birth time
- Permissions, UID, GID
- Inode, Hard links
- ETag (MD5), Checksums (if requested)

**S3 Object Output:**
- Name, Size, ETag
- Content-Type, Storage Class
- Last Modified, Expires
- Metadata, Encryption
- Checksums (CRC32, SHA1, SHA256 if available)

**S3 Bucket Output:**
- Bucket name and region
- Versioning status
- Encryption configuration

**Examples:**
```bash
hsc stat file.txt                                     # Local file info
hsc stat ./dir --recursive                            # All files in directory
hsc stat file.txt --checksum-mode ENABLED --checksum-algorithm SHA256
hsc stat s3://bucket/object.txt                       # S3 object info
hsc stat s3://bucket                                  # Bucket info
hsc stat s3://bucket/prefix/ --recursive              # All objects with prefix
```

### diff - Difference

Compare two directories or S3 locations.

```bash
hsc diff <source> <dest> [options]
```

**Options:**
- `--compare-content` - Compare by content (ETag/MD5) instead of just size
- `--include <pattern>` - Include only files matching pattern
- `--exclude <pattern>` - Exclude files from comparison

**Output Categories:**
- Only in source
- Only in destination
- Size differs
- Content differs (if --compare-content enabled)

**Examples:**
```bash
hsc diff ./local-dir s3://bucket/prefix/              # Compare by size
hsc diff --compare-content ./dir1 ./dir2              # Compare by content
hsc diff s3://bucket-a/data/ s3://bucket-b/data/      # Compare S3 locations
hsc diff --include "*.txt" ./docs s3://bucket/docs/
```

### cat - Concatenate

Output file or object content to stdout.

```bash
hsc cat <path> [options]
```

**Options:**
- `--range <start-end>` - Read specific byte range (e.g., "0-999" or "bytes=0-999")
- `--offset <bytes>` - Start reading from offset
- `--size <bytes>` - Read specific number of bytes

**Examples:**
```bash
hsc cat s3://bucket/file.txt                    # Print entire file
hsc cat file.txt --range 0-100                  # First 101 bytes
hsc cat s3://bucket/log.txt --offset 1000       # Skip first 1000 bytes
hsc cat file.txt --offset 100 --size 50         # Read bytes 100-149
hsc cat s3://bucket/data.txt | grep ERROR       # Pipe to other tools
```

## Filter Patterns

All commands that support `--include` and `--exclude` use glob patterns:

```bash
*.txt           # All .txt files
**/*.log        # All .log files in any subdirectory
data/202?.csv   # data/2020.csv, data/2021.csv, etc.
temp*           # Files starting with "temp"
```

**Pattern Behavior:**
- Multiple `--include` patterns: ANY match includes the file (OR logic)
- Multiple `--exclude` patterns: ANY match excludes the file (OR logic)
- Exclude takes precedence over include

## Multipart Upload

Configured in `~/.aws/config`:

```ini
[s3]
multipart_threshold = 10MB    # Files >= this size use multipart upload
multipart_chunksize = 5MB     # Size of each part
```

**Supported Size Formats:**
- Plain bytes: `8388608`
- Megabytes: `8MB` or `8M`
- Kilobytes: `5120KB` or `5120K`
- Gigabytes: `1GB` or `1G`

**Default Values:** 8MB for both threshold and chunksize

**Commands That Use Multipart:**
- `cp` - When uploading to S3
- `sync` - When uploading to S3
- `mv` - When moving to S3

## Environment Variable Precedence

Configuration is resolved in this order:

1. Command-line options (`--profile`, `--region`, `--endpoint-url`)
2. Environment variables (`AWS_PROFILE`, `AWS_REGION`, etc.)
3. AWS config files (`~/.aws/config`, `~/.aws/credentials`)
4. Built-in defaults

## Exit Codes

- `0` - Success
- `Non-zero` - Error occurred (error message printed to stderr)

## See Also

- [Main Documentation](../README.md)
- [Environment Variables](ENVIRONMENT.md)
- [Usage Examples](EXAMPLES.md)

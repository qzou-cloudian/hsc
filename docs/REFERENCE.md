# Command Reference

Quick reference for all hsc commands with detailed options.

## Global Options

Available for all commands:

```
--profile <name>             AWS profile to use
--region <region>            AWS region override
--endpoint-url <url>         Custom S3 endpoint (for S3-compatible services)
--no-verify-ssl              Disable SSL certificate verification
--debug                      Enable debug logging
--cli-connect-timeout <secs> TCP connect timeout in seconds (0 = no timeout)
--cli-read-timeout <secs>    Socket read timeout in seconds (0 = no timeout)
-H, --custom-header <KEY:VALUE>
                             Add a custom HTTP header to every request (repeatable)
--no-sign-request            Send requests without AWS signatures (for public buckets)
--rdma [PROVIDER]            Enable RDMA transfers (requires rdma feature build).
                             PROVIDER: auto (default), cuobj, mock
--version                    Show version information
```

## Commands

### mb - Make Bucket

Create a new S3 bucket.

```bash
hsc mb s3://bucket-name [--ignore-existing]
```

**Options:**
- `--ignore-existing` - Do not fail if the bucket already exists

**Examples:**
```bash
hsc mb s3://my-new-bucket
hsc --region eu-west-1 mb s3://eu-bucket
hsc mb s3://my-bucket --ignore-existing   # Idempotent create
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
- `--versions` - List all object versions and delete markers
- `--human-readable` - Format version-list sizes as KB/MB/GB
- `--json` - Emit structured JSON output

**Examples:**
```bash
hsc ls                              # List all buckets
hsc ls s3://bucket                  # List objects in bucket
hsc ls s3://bucket/prefix/          # List objects with prefix
hsc ls s3://bucket/ --recursive     # List all objects recursively
hsc ls s3://bucket/ --versions --json
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
- `--checksum [<alg>]` - Enable checksum for single file operations; optionally specify algorithm (CRC32, CRC32C, SHA1, SHA256); bare `--checksum` defaults to CRC32
- `--disable-multipart` - Always use a single PUT regardless of file size (max 5 GiB per object)
- `--part-size <SIZE>` - Set multipart threshold and chunk size (e.g. `16m`, `256m`, `1g`); conflicts with `--disable-multipart`

**Examples:**
```bash
hsc cp file.txt s3://bucket/                    # Upload file
hsc cp s3://bucket/file.txt ./                  # Download file
hsc cp --recursive ./dir s3://bucket/prefix/    # Upload directory
hsc cp --include "*.jpg" ./photos s3://bucket/  # Upload only .jpg files
hsc cp file.txt s3://bucket/ --checksum SHA256
hsc cp large.bin s3://bucket/ --part-size 64m   # Use 64 MiB parts
hsc cp small.txt s3://bucket/ --disable-multipart  # Force single PUT
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
- `--disable-multipart` - Always use a single PUT regardless of file size (max 5 GiB per object)
- `--part-size <SIZE>` - Set multipart threshold and chunk size (e.g. `16m`, `256m`, `1g`); conflicts with `--disable-multipart`

**Examples:**
```bash
hsc mv file.txt s3://bucket/newname.txt
hsc mv s3://bucket/old/ s3://bucket/new/ --recursive
hsc mv large.bin s3://bucket/large.bin --part-size 64m
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
- `--checksum [<alg>]` - Verify checksums during sync (CRC32, CRC32C, SHA1, SHA256)
- `--delete` - Delete destination objects/files not present in the source
- `--disable-multipart` - Always use a single PUT regardless of file size (max 5 GiB per object)
- `--part-size <SIZE>` - Set multipart threshold and chunk size (e.g. `16m`, `256m`, `1g`); conflicts with `--disable-multipart`

**Behavior:**
- Compares file sizes
- Only uploads/downloads files that are new or have changed
- More efficient than `cp` for incremental backups

**Examples:**
```bash
hsc sync ./local-dir s3://bucket/backup/        # Backup local to S3
hsc sync s3://bucket/data/ ./local-cache/       # Download updates
hsc sync --exclude "*.tmp" ./project s3://backup/
hsc sync --part-size 32m ./large-data/ s3://bucket/  # 32 MiB parts
hsc sync --disable-multipart ./small-files/ s3://bucket/  # Force single PUT
```

### stat - Statistics

Display detailed information about files, directories, objects, or buckets.

```bash
hsc stat <path> [options]
```

**Options:**
- `--recursive` - Process directories/prefixes recursively
- `--checksum [<alg>]` - Compute checksum for local files; optionally specify algorithm (CRC32, CRC32C, SHA1, SHA256); bare `--checksum` defaults to SHA256
- `--json` - Emit structured JSON output

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
hsc stat file.txt --checksum SHA256
hsc stat s3://bucket/object.txt                       # S3 object info
hsc stat s3://bucket                                  # Bucket info
hsc stat s3://bucket/prefix/ --recursive              # All objects with prefix
hsc stat file.txt --json                              # Machine-readable output
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
- `--json` - Emit structured JSON output

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
hsc diff --json ./dir1 ./dir2
```

### cmp - Compare

Compare two files or objects byte-by-byte. On success (identical), prints `identical: true` followed by both content hashes; on failure, prints `identical: false` and the first difference. Exits `0` when identical, `1` when they differ.

```bash
hsc cmp <path1> <path2> [options]
```

**Options:**
- `--algorithm <alg>` - Hash algorithm to compute when content matches: `MD5`, `CRC32`, `CRC32C`, `SHA1`, `SHA256` (default: `SHA256`). Skipped when `--range`/`--offset`/`--size` is set.
- `--range <start-end>` - Compare a specific byte range (e.g., `0-999` or `bytes=0-999`)
- `--offset <bytes>` - Start comparison from this byte offset
- `--size <bytes>` - Number of bytes to compare (used with `--offset`)
- `--sse-c <alg>` - SSE-C algorithm for S3 objects (AES256)
- `--sse-c-key <key>` - Base64-encoded SSE-C customer key
- `--json` - Emit structured JSON output

**Text output (identical):**
```
identical: true
algorithm: SHA256
/path/to/file: <hex>
s3://bucket/key: <hex>
```

**Text output (different):**
```
identical: false
reason: content differs at byte N, line M
```

**Exit Codes:**
- `0` - Files are identical (within the requested range)
- `1` - Files differ

**Examples:**
```bash
hsc cmp file.txt s3://bucket/file.txt            # Verify local matches S3, print hash
hsc cmp s3://bucket/a.bin s3://bucket/b.bin      # Compare two S3 objects
hsc cmp --range 0-999 a.bin b.bin                # Compare first 1000 bytes only
hsc cmp --offset 512 --size 256 a.bin b.bin      # Compare bytes 512-767
hsc cmp --algorithm MD5 a.bin b.bin              # Use MD5 for the success hash
hsc cmp --json a.bin s3://bucket/a.bin           # Machine-readable output
hsc cmp a.bin b.bin && echo "identical"          # Script usage
```

### exists - Exists

Test whether a local path, S3 bucket, or S3 object exists.

```bash
hsc exists <path> [--json]
```

**Options:**
- `--json` - Emit structured JSON output (`{"path": "...", "exists": true}`)

**Behavior:**
- Prints `true`/`false` in text mode
- Exits `0` when the target exists, `1` when it does not

**Examples:**
```bash
hsc exists ./file.txt
hsc exists s3://bucket/key
hsc exists --json s3://bucket
```

### hash - Hash

Compute a digest for a local file or S3 object by streaming its content.

```bash
hsc hash <path> [--algorithm <alg>] [--json]
```

**Options:**
- `--algorithm <alg>` - One of `MD5`, `CRC32`, `CRC32C`, `SHA1`, `SHA256` (default: `SHA256`)
- `--json` - Emit structured JSON output

**Examples:**
```bash
hsc hash ./file.bin
hsc hash s3://bucket/file.bin --algorithm MD5
hsc hash --json ./file.bin --algorithm SHA256
```

### parts - Parts

Show multipart upload metadata for an S3 object. Uses `HeadObject` by default (works on any S3-compatible server); pass `--attributes` to use the AWS `GetObjectAttributes` API which additionally returns per-part checksums.

```bash
hsc parts s3://bucket/key [--attributes] [--json]
```

**Options:**
- `--attributes` - Use `GetObjectAttributes` instead of `HeadObject` (AWS only; provides per-part checksums)
- `--json` - Emit structured JSON output

**Notes:**
- For single-put (non-multipart) objects the part list is empty and `Parts: 1 (single-put)` is shown.
- When `--attributes` is set, part count and per-part sizes are served directly by AWS without extra round-trips; the HeadObject path issues one request per part.

**Examples:**
```bash
hsc parts s3://bucket/large-object.bin              # Default (HeadObject)
hsc parts --attributes s3://bucket/large-object.bin # AWS path with checksums
hsc parts --json s3://bucket/large-object.bin
```

### cat - Concatenate

Output file or object content to stdout.

```bash
hsc cat <path> [options]
```

**Options:**
- `--range <start-end>` - Read specific byte range (e.g., "0-999" or "bytes=0-999")
- `--offset <bytes>` - Start reading from offset
- `--size <bytes>` - Read specific number of bytes (used with `--offset`)
- `--part-number <n>` - Download a specific part of a multipart-uploaded S3 object (1–10000)
- `--version-id <id>` - Return a specific version of an S3 object (requires bucket versioning)

**Notes:**
- `--part-number` and `--range`/`--offset`/`--size` are mutually exclusive
- `--part-number` and `--version-id` apply to S3 objects only

**Examples:**
```bash
hsc cat s3://bucket/file.txt                       # Print entire file
hsc cat file.txt --range 0-100                     # First 101 bytes
hsc cat s3://bucket/log.txt --offset 1000          # Skip first 1000 bytes
hsc cat file.txt --offset 100 --size 50            # Read bytes 100-149
hsc cat s3://bucket/data.bin --part-number 1       # First part of multipart upload
hsc cat s3://bucket/data.bin --part-number 3       # Third part only
hsc cat s3://bucket/file.txt --version-id abc123   # Specific version
hsc cat s3://bucket/data.txt | grep ERROR          # Pipe to other tools
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

Controlled via per-command CLI flags (highest priority) or `~/.aws/config` defaults.

| Flag | Effect |
|---|---|
| `--disable-multipart` | Always use single PUT; never split into parts (max 5 GiB) |
| `--part-size <SIZE>` | Use multipart for files ≥ SIZE; split into SIZE-byte parts |

`--disable-multipart` and `--part-size` are mutually exclusive.
Accepted size suffixes: `k`/`K` (KiB), `m`/`M` (MiB), `g`/`G` (GiB), or plain bytes.

When neither flag is given, values come from `~/.aws/config`:

```ini
[s3]
multipart_threshold = 10MB    # Files >= this size use multipart upload
multipart_chunksize = 5MB     # Size of each part
```

**Supported config size formats:** Plain bytes (`8388608`), `8MB`/`8M`, `5120KB`/`5120K`, `1GB`/`1G`

**Default:** 8 MiB for both threshold and chunksize

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
- `1` - Content differs (`cmp`), or target does not exist (`exists`)
- `Non-zero` - Error occurred (error message printed to stderr)

## See Also

- [Main Documentation](../README.md)
- [Environment Variables](ENVIRONMENT.md)
- [Usage Examples](EXAMPLES.md)

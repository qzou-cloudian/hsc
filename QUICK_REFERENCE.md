# hsc Quick Reference Card

## Installation
```bash
cargo install hsc
```

## Common Commands

### Bucket Operations
```bash
hsc mb s3://bucket                    # Create bucket
hsc rb s3://bucket --force            # Delete bucket + contents
hsc ls                                # List all buckets
hsc ls s3://bucket/                   # List objects in bucket
```

### Upload/Download
```bash
hsc cp file.txt s3://bucket/          # Upload file
hsc cp s3://bucket/file.txt ./        # Download file
hsc cp -r ./dir s3://bucket/prefix/   # Upload directory
hsc cp -r s3://bucket/prefix/ ./dir/  # Download directory
```

### Sync & Move
```bash
hsc sync ./local s3://bucket/backup/  # Smart upload (changed files only)
hsc sync s3://bucket/data/ ./cache/   # Smart download
hsc mv old.txt s3://bucket/new.txt    # Move (upload + delete)
```

### Information
```bash
hsc stat file.txt                     # Local file info + MD5
hsc stat s3://bucket/object.txt       # S3 object metadata
hsc stat s3://bucket                  # Bucket info
hsc diff ./local s3://bucket/remote/  # Compare directories
```

### Content Operations
```bash
hsc cat s3://bucket/file.txt          # Print to stdout
hsc cat file.txt --range 0-999        # First 1000 bytes
hsc cat s3://log.txt | grep ERROR     # Pipe to other tools
hsc rm s3://bucket/file.txt           # Delete object
hsc rm s3://bucket/logs/ -r           # Delete all with prefix
```

## Filtering
```bash
--include "*.txt"                        # Include pattern
--exclude "*.log"                        # Exclude pattern
--include "*.jpg" --include "*.png"      # Multiple includes (OR)
--exclude "temp*" --exclude "*.tmp"      # Multiple excludes (OR)
```

## Global Options
```bash
--profile production                     # Use specific AWS profile
--region us-west-2                       # Override region
--endpoint-url https://s3.example.com    # Use S3-compatible service
--debug                                  # Enable debug output
--no-verify-ssl                          # Disable SSL verification
```

## Configuration

### AWS Credentials (~/.aws/credentials)
```ini
[default]
aws_access_key_id = YOUR_KEY
aws_secret_access_key = YOUR_SECRET
```

### AWS Config (~/.aws/config)
```ini
[default]
region = us-east-1

[s3]
multipart_threshold = 10MB
multipart_chunksize = 5MB
```

## Environment Variables
```bash
export AWS_PROFILE=production
export AWS_REGION=us-west-2
export AWS_ENDPOINT_URL=https://s3.example.com
export AWS_ACCESS_KEY_ID=key
export AWS_SECRET_ACCESS_KEY=secret
```

## Advanced Examples

### Backup with filters
```bash
hsc sync --exclude "*.tmp" --exclude ".git/*" \
  ./project s3://backups/project/
```

### Verify file integrity
```bash
hsc cp large.zip s3://bucket/ \
  --checksum-algorithm SHA256
```

### Compare by content
```bash
hsc diff --compare-content \
  s3://prod-bucket/data/ s3://staging-bucket/data/
```

### Read specific range
```bash
# Get last 1KB of log file
hsc cat s3://logs/app.log \
  --offset $(expr $(hsc stat s3://logs/app.log | grep Size | awk '{print $3}') - 1024)
```

### Recursive stat with checksums
```bash
hsc stat ./dir --recursive \
  --checksum-mode ENABLED --checksum-algorithm SHA256
```

## Tips

- **Sync vs Copy**: Use `sync` for incremental backups (faster for large directories)
- **Multipart**: Automatically used for files >= 8MB (configurable)
- **Checksums**: Add `--checksum-algorithm` for data integrity verification
- **Ranges**: Use `cat --range` to inspect large files without downloading entirely
- **Filters**: Combine `--include` and `--exclude` for fine-grained control
- **Endpoints**: Set `AWS_ENDPOINT_URL` for MinIO, Cloudian, or other S3-compatible services

## Troubleshooting

### Credentials not found
```bash
# Check credentials file
cat ~/.aws/credentials

# Or use environment variables
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
```

### Custom endpoint not working
```bash
# Make sure to include protocol
hsc --endpoint-url https://s3.example.com ls

# For self-signed certs
hsc --no-verify-ssl --endpoint-url https://s3.example.com ls
```

### Slow uploads
```bash
# Check multipart settings in ~/.aws/config
[s3]
multipart_threshold = 5MB    # Lower threshold for faster multipart
multipart_chunksize = 5MB    # Adjust chunk size
```

## Getting Help
```bash
hsc --help                # General help
hsc cp --help             # Command-specific help
hsc --version             # Show version
```

## More Information
- [Full documentation](README.md)
- [Command reference](docs/REFERENCE.md)
- [Environment Variables](doncs/ENVIRONMENT.md)
- [Usage Examples](docs/EXAMPLES.md)

# Usage Examples

This document provides practical examples for using hsc with various AWS configurations.

## Basic Examples

### List All Buckets
```bash
hsc ls
```

### List Objects in a Bucket
```bash
hsc ls s3://my-bucket
hsc ls s3://my-bucket --recursive
```

### Upload a File
```bash
hsc cp local-file.txt s3://my-bucket/remote-file.txt
```

### Download a File
```bash
hsc cp s3://my-bucket/file.txt local-file.txt
```

## Using Global Options

### Debug Mode
See what the tool is doing:
```bash
hsc --debug ls
```

Output:
```
Debug: Using AWS profile: default
Debug: Using region: us-east-1
Debug: S3 client initialized successfully
```

### Custom Endpoint (MinIO, Cloudian, etc.)
```bash
# Via CLI option
hsc --endpoint-url http://minio:9000 ls

# Via environment variable
export AWS_ENDPOINT_URL=http://minio:9000
hsc ls
```

### Specify Region
```bash
# Via CLI option
hsc --region eu-west-1 ls

# Via environment variable
export AWS_REGION=eu-west-1
hsc ls
```

### Use Specific AWS Profile
```bash
# Via CLI option
hsc --profile production ls

# Via environment variable
export AWS_PROFILE=production
hsc ls
```

### Combine Multiple Options
```bash
hsc --debug \
       --profile dev \
       --region us-west-2 \
       --endpoint-url http://minio:9000 \
       ls
```

## Working with Profiles

### Setup
Create `~/.aws/credentials`:
```ini
[default]
aws_access_key_id = DEFAULT_KEY
aws_secret_access_key = DEFAULT_SECRET

[production]
aws_access_key_id = PROD_KEY
aws_secret_access_key = PROD_SECRET

[dev]
aws_access_key_id = DEV_KEY
aws_secret_access_key = DEV_SECRET
```

Create `~/.aws/config`:
```ini
[default]
region = us-east-1

[profile production]
region = us-east-1

[profile dev]
region = us-west-2
endpoint_url = http://localhost:9000
```

### Use Different Profiles
```bash
# Use production profile
hsc --profile production cp backup.tar.gz s3://prod-backups/

# Use dev profile with local MinIO
hsc --profile dev mb s3://test-bucket

# Check which profile is active
hsc --debug --profile production ls
```

## Environment Variables

### Basic Setup
```bash
export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE
export AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
export AWS_REGION=us-east-1

hsc ls
```

### Custom Config Files
```bash
export AWS_CONFIG_FILE=/path/to/my/config
export AWS_SHARED_CREDENTIALS_FILE=/path/to/my/credentials
export AWS_PROFILE=myprofile

hsc ls
```

### Temporary Credentials (STS)
```bash
export AWS_ACCESS_KEY_ID=ASIAIOSFODNN7EXAMPLE
export AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
export AWS_SESSION_TOKEN=AQoEXAMPLEH4aoAH0gNCAPyJxz4BlCFFxWNE1OPTgk5...

hsc cp file.txt s3://bucket/file.txt
```

## S3-Compatible Services

### MinIO
```bash
export AWS_ENDPOINT_URL=http://localhost:9000
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_REGION=us-east-1

# Create bucket
hsc mb s3://test-bucket

# Upload files
hsc cp --recursive ./data/ s3://test-bucket/data/

# List
hsc ls s3://test-bucket --recursive
```

### Cloudian
```bash
export AWS_ENDPOINT_URL=http://cloudian-host:8080
export AWS_ACCESS_KEY_ID=your_key
export AWS_SECRET_ACCESS_KEY=your_secret

hsc --debug ls
```

### Ceph RADOS Gateway
```bash
hsc --endpoint-url http://radosgw:8080 \
       --region default \
       ls
```

## Advanced Copy Examples

### Upload Directory with Filters
```bash
# Upload only .txt files, exclude .log files
hsc cp --recursive \
       --include "*.txt" \
       --exclude "*.log" \
       ./documents/ \
       s3://my-bucket/documents/
```

### Copy with Checksum Validation
```bash
hsc cp important-file.dat s3://my-bucket/important-file.dat \
       --checksum-mode ENABLED \
       --checksum-algorithm SHA256
```

### Large Directory Upload
```bash
# Upload large directory recursively
hsc cp --recursive \
       --debug \
       ./large-dataset/ \
       s3://data-bucket/dataset-$(date +%Y%m%d)/
```

## Sync Examples

### Backup to S3
```bash
# Initial backup
hsc sync ./important-data/ s3://backup-bucket/daily/

# Subsequent syncs only transfer changed files
hsc sync ./important-data/ s3://backup-bucket/daily/
```

### Sync with Filters
```bash
# Only sync images
hsc sync --include "*.jpg" \
            --include "*.png" \
            ./photos/ \
            s3://photo-bucket/
```

### Mirror S3 to Local
```bash
hsc sync s3://source-bucket/ ./local-mirror/
```

## Move and Organize

### Rename in S3
```bash
hsc mv s3://my-bucket/old-name.txt s3://my-bucket/new-name.txt
```

### Reorganize Directory Structure
```bash
hsc mv --recursive \
       s3://my-bucket/old-structure/ \
       s3://my-bucket/new-structure/
```

### Move with Filters
```bash
# Move only .log files to archive
hsc mv --recursive \
       --include "*.log" \
       s3://logs-bucket/current/ \
       s3://logs-bucket/archive/$(date +%Y-%m)/
```

## Remove Examples

### Delete Single File
```bash
hsc rm s3://my-bucket/file.txt
```

### Delete Directory
```bash
hsc rm --recursive s3://my-bucket/old-directory/
```

### Clean Up Old Files
```bash
# Remove all .tmp files
hsc rm --recursive \
       --include "*.tmp" \
       s3://temp-bucket/
```

## Bucket Management

### Create Bucket
```bash
hsc mb s3://new-bucket-name
```

### Delete Empty Bucket
```bash
hsc rb s3://old-bucket
```

### Force Delete Non-Empty Bucket
```bash
hsc rb --force s3://bucket-with-data
```

## Multi-Region Examples

### Copy Between Regions
```bash
# Copy from us-east-1 to eu-west-1
hsc --region us-east-1 cp \
       s3://us-bucket/file.txt \
       s3://eu-bucket/file.txt \
       --debug

# Or set region for destination
AWS_REGION=eu-west-1 hsc cp \
       s3://us-bucket/file.txt \
       s3://eu-bucket/file.txt
```

### Sync Across Regions
```bash
# Sync from US to EU bucket
hsc --region us-east-1 sync \
       s3://us-primary/ \
       s3://eu-replica/
```

## Scripting Examples

### Automated Backup Script
```bash
#!/bin/bash
BACKUP_DATE=$(date +%Y%m%d)
SOURCE_DIR="/data/important"
S3_BUCKET="s3://backups"

# Debug mode for logging
hsc --debug sync \
       "$SOURCE_DIR" \
       "$S3_BUCKET/daily/$BACKUP_DATE/" \
       2>&1 | tee backup.log

echo "Backup completed: $BACKUP_DATE"
```

### Multi-Environment Deploy
```bash
#!/bin/bash
ENVIRONMENTS="dev staging production"

for ENV in $ENVIRONMENTS; do
    echo "Deploying to $ENV..."
    hsc --profile "$ENV" \
           --region us-east-1 \
           cp --recursive \
           ./dist/ \
           s3://"${ENV}"-app-bucket/v$(cat VERSION)/
done
```

### Cleanup Old Backups
```bash
#!/bin/bash
# Keep last 7 daily backups

BUCKET="s3://backups/daily"
KEEP_DAYS=7

hsc ls "$BUCKET" --recursive | \
  grep -E '[0-9]{8}' | \
  awk '{print $4}' | \
  sort -r | \
  tail -n +$((KEEP_DAYS + 1)) | \
  while read OLD_BACKUP; do
    echo "Removing old backup: $OLD_BACKUP"
    hsc rm --recursive "$BUCKET/$OLD_BACKUP/"
  done
```

## Troubleshooting Examples

### Verify Credentials
```bash
# Test with debug mode
hsc --debug ls
```

### Test Endpoint Connectivity
```bash
# Test custom endpoint
hsc --debug \
       --endpoint-url http://minio:9000 \
       --region us-east-1 \
       ls

# Check connectivity separately
curl http://minio:9000
```

### Check Which Config is Used
```bash
# Show all debug info
hsc --debug \
       --profile myprofile \
       --region us-west-2 \
       ls 2>&1 | grep "Debug:"
```

### Verify File Upload
```bash
# Upload with debug
hsc --debug cp large-file.bin s3://bucket/large-file.bin

# Verify size matches
hsc ls s3://bucket/ | grep large-file.bin
```

## Performance Tips

### Parallel Operations
While the tool processes files sequentially, you can run multiple instances:
```bash
# Upload different directories in parallel
hsc cp --recursive dir1/ s3://bucket/dir1/ &
hsc cp --recursive dir2/ s3://bucket/dir2/ &
hsc cp --recursive dir3/ s3://bucket/dir3/ &
wait
```

### Large File Transfers
```bash
# Use sync for large datasets (only transfers changes)
hsc sync ./large-dataset/ s3://data-bucket/

# For full uploads, use cp with debug to monitor
hsc --debug cp --recursive ./data/ s3://bucket/data/
```

## Integration Examples

### CI/CD Pipeline
```yaml
# .github/workflows/deploy.yml
- name: Deploy to S3
  run: |
    export AWS_ACCESS_KEY_ID=${{ secrets.AWS_KEY }}
    export AWS_SECRET_ACCESS_KEY=${{ secrets.AWS_SECRET }}
    export AWS_REGION=us-east-1
    
    hsc sync --recursive \
           ./build/ \
           s3://${{ secrets.S3_BUCKET }}/
```

### Cron Job
```bash
# /etc/cron.d/s3-backup
0 2 * * * user hsc --profile backup sync /data/db-dumps/ s3://backups/db/
```

### Docker
```dockerfile
FROM rust:1.75 as builder
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /target/release/hsc /usr/local/bin/
ENV AWS_REGION=us-east-1
CMD ["hsc", "--help"]
```

## Best Practices

1. **Use debug mode** when troubleshooting:
   ```bash
   hsc --debug <command>
   ```

2. **Use sync for backups** to save bandwidth:
   ```bash
   hsc sync ./data/ s3://backups/
   ```

3. **Use profiles** for different environments:
   ```bash
   hsc --profile production ...
   ```

4. **Use filters** to be selective:
   ```bash
   hsc cp --recursive --include "*.jpg" ./photos/ s3://bucket/
   ```

5. **Test with non-production** first:
   ```bash
   hsc --profile dev mb s3://test-bucket
   ```

## Stat Command Examples

### Local Filesystem Stat

```bash
# Get information about a file
hsc stat /path/to/file.txt
```

Output:
```
Name      : /path/to/file.txt
Type      : file
Size      : 1024 bytes (1.00 KB)
Modified  : 2026-02-25 16:21:03 UTC
Accessed  : 2026-02-25 16:21:06 UTC
Mode      : 644
UID       : 1000
GID       : 1000
Inode     : 12345678
Links     : 1
```

### Directory Stat

```bash
# Get information about a directory
hsc stat /path/to/directory/
```

Output:
```
Name      : /path/to/directory/
Type      : directory
Modified  : 2026-02-25 10:30:45 UTC
Accessed  : 2026-02-25 12:15:30 UTC
Mode      : 755
UID       : 1000
GID       : 1000
Inode     : 87654321
Links     : 5
```

### S3 Object Stat

```bash
# Get detailed information about an S3 object
hsc stat s3://my-bucket/important-file.pdf
```

Output:
```
Name      : s3://my-bucket/important-file.pdf
Type      : file
Size      : 2048576 bytes (2000.00 KB)
Modified  : 2026-02-25T14:30:00Z
ETag      : "abc123def456..."
Content   : application/pdf
Storage   : STANDARD
SHA256    : abc123...
Encryption: AES256

Metadata  :
  author: John Doe
  department: Engineering
```

### S3 Bucket Stat

```bash
# Get bucket information
hsc stat s3://my-production-bucket
```

Output:
```
Name      : my-production-bucket
Type      : s3 bucket
Status    : exists
Region    : us-east-1
Versioning: Enabled
Encryption: Enabled
Objects   : 1 (at least)
```

### Comparing Local and S3

```bash
# Check local file before upload
hsc stat local-backup.tar.gz

# Upload to S3
hsc cp local-backup.tar.gz s3://backups/backup-$(date +%Y%m%d).tar.gz

# Verify S3 upload
hsc stat s3://backups/backup-$(date +%Y%m%d).tar.gz
```

### Scripting with Stat

```bash
#!/bin/bash
# Check if S3 object exists before downloading

OBJECT="s3://my-bucket/important-file.dat"

if hsc stat "$OBJECT" &>/dev/null; then
    echo "Object exists, downloading..."
    hsc cp "$OBJECT" ./important-file.dat
else
    echo "Object not found!"
    exit 1
fi
```

### Verify Checksums

```bash
# Upload with checksum
hsc cp important.dat s3://bucket/important.dat \
    --checksum-mode ENABLED --checksum-algorithm SHA256

# Check the checksum was stored
hsc stat s3://bucket/important.dat | grep SHA256
```

### Monitor File Changes

```bash
#!/bin/bash
# Monitor file modifications

watch -n 60 "hsc stat /var/log/application.log | grep Modified"
```

### Check Storage Class

```bash
# Check storage class of archived objects
hsc stat s3://archive-bucket/old-data.zip | grep Storage
```

### Audit Bucket Configuration

```bash
#!/bin/bash
# Audit all buckets for encryption

hsc ls | tail -n +2 | head -n -1 | awk '{print $2}' | while read BUCKET; do
    echo "Checking bucket: $BUCKET"
    hsc stat "s3://$BUCKET" | grep -E "Encryption|Versioning"
    echo ""
done
```


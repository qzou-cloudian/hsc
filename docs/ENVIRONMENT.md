# Environment Variables and Configuration

This document describes how hsc uses AWS environment variables and configuration files.

## Supported Environment Variables

### AWS Credentials

- **AWS_ACCESS_KEY_ID**: Your AWS access key ID
- **AWS_SECRET_ACCESS_KEY**: Your AWS secret access key  
- **AWS_SESSION_TOKEN**: Session token for temporary credentials (optional)

Example:
```bash
export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE
export AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
export AWS_SESSION_TOKEN=AQoEXAMPLEH4aoAH0gNCAPyJxz4BlCFFxWNE1OPTgk5TthT+FvwqnKwRcOIfrRh3c/L...
```

### AWS Configuration

- **AWS_CONFIG_FILE**: Path to AWS config file (default: `~/.aws/config`)
- **AWS_SHARED_CREDENTIALS_FILE**: Path to AWS credentials file (default: `~/.aws/credentials`)
- **AWS_PROFILE**: Which AWS profile to use (default: "default")
- **AWS_REGION**: AWS region (e.g., us-east-1, eu-west-1)
- **AWS_ENDPOINT_URL**: Custom S3 endpoint URL for S3-compatible services

Example:
```bash
export AWS_CONFIG_FILE=/path/to/config
export AWS_SHARED_CREDENTIALS_FILE=/path/to/credentials
export AWS_PROFILE=production
export AWS_REGION=us-west-2
export AWS_ENDPOINT_URL=http://minio:9000
```

## Configuration Precedence

Settings are applied in the following order (highest to lowest priority):

1. **CLI Options** (e.g., `--region`, `--profile`, `--endpoint-url`)
2. **Environment Variables** (e.g., `AWS_REGION`, `AWS_PROFILE`)
3. **Config Files** (`~/.aws/config` and `~/.aws/credentials`)
4. **Defaults** (profile: "default", no region set)

### Example Precedence

```bash
# Environment sets region to us-west-2
export AWS_REGION=us-west-2

# CLI option overrides environment
hsc --region eu-west-1 ls
# Uses eu-west-1 (CLI wins)

# Without CLI option
hsc ls
# Uses us-west-2 (from environment)
```

## AWS Config File Format

Standard AWS config file (`~/.aws/config`):

```ini
[default]
region = us-east-1
output = json

[profile dev]
region = us-west-2
output = json
endpoint_url = http://minio:9000
```

## AWS Credentials File Format

Standard AWS credentials file (`~/.aws/credentials`):

```ini
[default]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY

[dev]
aws_access_key_id = AKIAI44QH8DHBEXAMPLE
aws_secret_access_key = je7MtGbClwBF/2Zp9Utk/h3yCo8nvbEXAMPLEKEY
```

## S3-Compatible Endpoints

### Using Environment Variables

```bash
export AWS_ENDPOINT_URL=http://localhost:9000
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_REGION=us-east-1

hsc ls
```

### Using CLI Options

```bash
hsc --endpoint-url http://localhost:9000 \
       --region us-east-1 \
       ls
```

### Common S3-Compatible Services

**MinIO:**
```bash
export AWS_ENDPOINT_URL=http://localhost:9000
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
```

**Cloudian:**
```bash
export AWS_ENDPOINT_URL=http://cloudian-host:8080
export AWS_ACCESS_KEY_ID=your_access_key
export AWS_SECRET_ACCESS_KEY=your_secret_key
```

**Ceph RADOS Gateway:**
```bash
export AWS_ENDPOINT_URL=http://radosgw-host:8080
export AWS_ACCESS_KEY_ID=your_access_key
export AWS_SECRET_ACCESS_KEY=your_secret_key
```

## Using Multiple Profiles

### Setup Profiles

Create `~/.aws/credentials`:
```ini
[default]
aws_access_key_id = DEFAULT_KEY
aws_secret_access_key = DEFAULT_SECRET

[production]
aws_access_key_id = PROD_KEY
aws_secret_access_key = PROD_SECRET

[staging]
aws_access_key_id = STAGING_KEY
aws_secret_access_key = STAGING_SECRET
```

Create `~/.aws/config`:
```ini
[default]
region = us-east-1

[profile production]
region = us-east-1

[profile staging]
region = us-west-2
```

### Use Specific Profile

```bash
# Via environment variable
export AWS_PROFILE=production
hsc ls

# Via CLI option
hsc --profile production ls

# Via CLI option (overrides environment)
export AWS_PROFILE=staging
hsc --profile production ls  # Uses production
```

## Debug Mode

Enable debug output to see which settings are being used:

```bash
hsc --debug ls
```

Output example:
```
Debug: Using AWS profile: production
Debug: Using region: us-east-1
Debug: Using custom endpoint: http://minio:9000
Debug: S3 client initialized successfully
```

## Temporary Credentials

For temporary credentials (e.g., from AWS STS):

```bash
export AWS_ACCESS_KEY_ID=ASIAIOSFODNN7EXAMPLE
export AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
export AWS_SESSION_TOKEN=AQoEXAMPLEH4aoAH0gNCAPyJxz4BlCFFxWNE1OPTgk5...

hsc ls
```

## Troubleshooting

### Check Which Settings Are Active

Use `--debug` to see active settings:
```bash
hsc --debug ls 2>&1 | grep "Debug:"
```

### Verify Config Files Are Loaded

```bash
# Set specific paths
export AWS_CONFIG_FILE=/path/to/your/config
export AWS_SHARED_CREDENTIALS_FILE=/path/to/your/credentials

# Verify with debug
hsc --debug --profile myprofile ls
```

### Test Endpoint Connectivity

```bash
# Test with debug mode
hsc --debug --endpoint-url http://endpoint:9000 ls

# Check if endpoint is reachable
curl http://endpoint:9000
```

## Security Best Practices

1. **Never commit credentials** to version control
2. **Use IAM roles** when running on EC2/ECS
3. **Use temporary credentials** when possible
4. **Restrict file permissions** on credential files:
   ```bash
   chmod 600 ~/.aws/credentials
   chmod 600 ~/.aws/config
   ```
5. **Rotate credentials** regularly
6. **Use separate profiles** for different environments

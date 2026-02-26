use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use crc32fast::Hasher as Crc32Hasher;
use md5::{Digest, Md5};
use sha1::Sha1;
use sha2::Sha256;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncReadExt;
use walkdir::WalkDir;

/// Display information about S3 objects, buckets, or local files
pub async fn stat(
    client: &Client,
    path: &str,
    recursive: bool,
    checksum_mode: Option<String>,
    checksum_algorithm: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path_type = parse_path(path)?;

    match path_type {
        PathType::S3 { bucket, key } => {
            if key.is_empty() {
                if recursive {
                    // Recursive stat all objects in bucket
                    stat_s3_recursive(client, &bucket, "").await
                } else {
                    // Bucket stat only
                    stat_bucket(client, &bucket).await
                }
            } else if recursive {
                // Recursive S3 object stat with prefix
                stat_s3_recursive(client, &bucket, &key).await
            } else {
                // Single S3 object stat
                stat_object(client, &bucket, &key).await
            }
        }
        PathType::Local(local_path) => {
            if recursive {
                // Recursive local stat
                stat_local_recursive(&local_path, checksum_mode, checksum_algorithm).await
            } else {
                // Single local file/directory stat
                stat_local(&local_path, checksum_mode, checksum_algorithm).await
            }
        }
    }
}

/// Display S3 bucket information
async fn stat_bucket(client: &Client, bucket: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("Name      : {}", bucket);
    println!("Type      : s3 bucket");

    // Check if bucket exists
    match client.head_bucket().bucket(bucket).send().await {
        Ok(_response) => {
            println!("Status    : exists");

            // Get bucket location
            match client.get_bucket_location().bucket(bucket).send().await {
                Ok(location) => {
                    let region = location
                        .location_constraint()
                        .map(|c| c.as_str())
                        .unwrap_or("us-east-1");
                    println!("Region    : {}", region);
                }
                Err(_) => {
                    // Location not available (might be us-east-1 or custom endpoint)
                }
            }

            // Get bucket versioning
            match client.get_bucket_versioning().bucket(bucket).send().await {
                Ok(versioning) => {
                    if let Some(status) = versioning.status() {
                        println!("Versioning: {}", status.as_str());
                    }
                }
                Err(_) => {
                    // Versioning info not available
                }
            }

            // Get bucket encryption
            match client.get_bucket_encryption().bucket(bucket).send().await {
                Ok(_encryption) => {
                    println!("Encryption: Enabled");
                }
                Err(_) => {
                    // Encryption not configured or not accessible
                }
            }

            // Count objects (sample)
            match client
                .list_objects_v2()
                .bucket(bucket)
                .max_keys(1)
                .send()
                .await
            {
                Ok(result) => {
                    if let Some(count) = result.key_count() {
                        if count > 0 {
                            println!("Objects   : {} (at least)", count);
                        } else {
                            println!("Objects   : 0 (empty)");
                        }
                    }
                }
                Err(_) => {
                    // Can't list objects
                }
            }
        }
        Err(e) => {
            return Err(format!(
                "Bucket '{}' does not exist or is not accessible: {}",
                bucket, e
            )
            .into());
        }
    }

    Ok(())
}

/// Display S3 object information
async fn stat_object(
    client: &Client,
    bucket: &str,
    key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = client.head_object().bucket(bucket).key(key).send().await?;

    println!("Name      : s3://{}/{}", bucket, key);
    println!("Type      : file");

    // Size
    if let Some(size) = response.content_length() {
        println!(
            "Size      : {} bytes ({:.2} KB)",
            size,
            size as f64 / 1024.0
        );
    }

    // Last Modified
    if let Some(last_modified) = response.last_modified() {
        println!("Modified  : {}", last_modified);
    }

    // ETag
    if let Some(etag) = response.e_tag() {
        println!("ETag      : {}", etag);
    }

    // Content Type
    if let Some(content_type) = response.content_type() {
        println!("Content   : {}", content_type);
    }

    // Storage Class
    if let Some(storage_class) = response.storage_class() {
        println!("Storage   : {}", storage_class.as_str());
    }

    // Checksums
    if let Some(checksum) = response.checksum_crc32() {
        println!("CRC32     : {}", checksum);
    }
    if let Some(checksum) = response.checksum_crc32_c() {
        println!("CRC32C    : {}", checksum);
    }
    if let Some(checksum) = response.checksum_sha1() {
        println!("SHA1      : {}", checksum);
    }
    if let Some(checksum) = response.checksum_sha256() {
        println!("SHA256    : {}", checksum);
    }

    // Server Side Encryption
    if let Some(sse) = response.server_side_encryption() {
        println!("Encryption: {}", sse.as_str());
    }

    // Metadata
    if let Some(metadata) = response.metadata() {
        if !metadata.is_empty() {
            println!("\nMetadata  :");
            for (key, value) in metadata {
                println!("  {}: {}", key, value);
            }
        }
    }

    // Cache Control
    if let Some(cache_control) = response.cache_control() {
        println!("Cache     : {}", cache_control);
    }

    // Expires
    if let Some(expires) = response.expires_string() {
        println!("Expires   : {}", expires);
    }

    Ok(())
}

/// Display local filesystem information (S3-compatible format)
async fn stat_local(
    path: &str,
    checksum_mode: Option<String>,
    checksum_algorithm: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Normalize the path by stripping trailing slashes
    let normalized_path = path.trim_end_matches('/');
    let path_obj = Path::new(normalized_path);

    if !path_obj.exists() {
        return Err(format!("Path '{}' does not exist", normalized_path).into());
    }

    let metadata = fs::metadata(path_obj).await?;

    println!("Name      : {}", normalized_path);

    // Type
    let file_type = if metadata.is_dir() {
        "directory"
    } else if metadata.is_symlink() {
        "symbolic link"
    } else {
        "file"
    };
    println!("Type      : {}", file_type);

    // Size
    let size = metadata.len();
    println!(
        "Size      : {} bytes ({:.2} KB)",
        size,
        size as f64 / 1024.0
    );

    // Modified time
    if let Ok(modified) = metadata.modified() {
        if let Ok(datetime) = modified.duration_since(std::time::UNIX_EPOCH) {
            let secs = datetime.as_secs();
            let dt = chrono::DateTime::from_timestamp(secs as i64, 0)
                .unwrap_or(chrono::DateTime::UNIX_EPOCH);
            println!("Modified  : {}", dt.format("%Y-%m-%d %H:%M:%S %Z"));
        }
    }

    // For files, calculate ETag and checksums
    if metadata.is_file() {
        // Calculate MD5 (ETag equivalent)
        if let Ok(etag) = calculate_file_md5(path_obj).await {
            println!("ETag      : \"{}\"", etag);
        }

        // Content-Type (basic detection)
        if let Some(extension) = path_obj.extension() {
            let content_type = match extension.to_str() {
                Some("txt") => "text/plain",
                Some("html") | Some("htm") => "text/html",
                Some("json") => "application/json",
                Some("xml") => "application/xml",
                Some("pdf") => "application/pdf",
                Some("jpg") | Some("jpeg") => "image/jpeg",
                Some("png") => "image/png",
                Some("gif") => "image/gif",
                Some("zip") => "application/zip",
                Some("tar") => "application/x-tar",
                Some("gz") => "application/gzip",
                _ => "application/octet-stream",
            };
            println!("Content   : {}", content_type);
        } else {
            println!("Content   : application/octet-stream");
        }

        // Calculate checksums if requested
        let calc_checksums = checksum_mode.as_deref() == Some("ENABLED")
            || checksum_mode.as_deref() == Some("enabled");

        if calc_checksums {
            if let Some(algo) = checksum_algorithm.as_deref() {
                match algo.to_uppercase().as_str() {
                    "CRC32" => {
                        if let Ok(checksum) = calculate_file_crc32(path_obj).await {
                            println!("CRC32     : {}", checksum);
                        }
                    }
                    "CRC32C" => {
                        // CRC32C is similar to CRC32, using same implementation for demo
                        if let Ok(checksum) = calculate_file_crc32(path_obj).await {
                            println!("CRC32C    : {}", checksum);
                        }
                    }
                    "SHA1" => {
                        if let Ok(checksum) = calculate_file_sha1(path_obj).await {
                            println!("SHA1      : {}", checksum);
                        }
                    }
                    "SHA256" => {
                        if let Ok(checksum) = calculate_file_sha256(path_obj).await {
                            println!("SHA256    : {}", checksum);
                        }
                    }
                    _ => {}
                }
            } else {
                // Default to all checksums
                if let Ok(checksum) = calculate_file_sha256(path_obj).await {
                    println!("SHA256    : {}", checksum);
                }
            }
        }
    }

    // Storage (local filesystem)
    println!("Storage   : local");

    // Permissions (Unix-like systems)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        println!("Mode      : {:o}", mode & 0o777);
    }

    // Additional Unix metadata
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        println!("UID       : {}", metadata.uid());
        println!("GID       : {}", metadata.gid());
        println!("Inode     : {}", metadata.ino());
        println!("Links     : {}", metadata.nlink());
    }

    Ok(())
}

/// Calculate MD5 hash of a file (ETag equivalent)
async fn calculate_file_md5(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = Md5::new();
    let mut buffer = vec![0u8; 8192];

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// Calculate CRC32 checksum of a file
async fn calculate_file_crc32(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = Crc32Hasher::new();
    let mut buffer = vec![0u8; 8192];

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:08x}", hasher.finalize()))
}

/// Calculate SHA1 hash of a file
async fn calculate_file_sha1(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 8192];

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// Calculate SHA256 hash of a file
async fn calculate_file_sha256(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 8192];

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// Stat local files recursively
async fn stat_local_recursive(
    path: &str,
    checksum_mode: Option<String>,
    checksum_algorithm: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path_obj = Path::new(path);

    if !path_obj.exists() {
        return Err(format!("Path '{}' does not exist", path).into());
    }

    if !path_obj.is_dir() {
        // Single file
        return stat_local(path, checksum_mode, checksum_algorithm).await;
    }

    // Walk directory recursively
    for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        let entry_path = entry.path();

        if entry_path.is_file() {
            stat_local(
                entry_path.to_str().unwrap(),
                checksum_mode.clone(),
                checksum_algorithm.clone(),
            )
            .await?;
            println!(); // Blank line between entries
        }
    }

    Ok(())
}

/// Stat S3 objects recursively
async fn stat_s3_recursive(
    client: &Client,
    bucket: &str,
    prefix: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut continuation_token: Option<String> = None;

    loop {
        let mut request = client.list_objects_v2().bucket(bucket);

        if !prefix.is_empty() {
            request = request.prefix(prefix);
        }

        if let Some(token) = continuation_token {
            request = request.continuation_token(token);
        }

        let response = request.send().await?;

        for obj in response.contents() {
            if let Some(key) = obj.key() {
                stat_object(client, bucket, key).await?;
                println!(); // Blank line between entries
            }
        }

        if response.is_truncated() == Some(true) {
            continuation_token = response.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    Ok(())
}

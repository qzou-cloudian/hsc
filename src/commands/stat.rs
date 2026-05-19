use crate::commands::hash::{compute_hash_for_local_path, HashAlgorithm};
use crate::commands::listing::{list_s3_keys, s3_prefix_has_objects, walk_local_files};
use crate::commands::object_metadata::ObjectChecksums;
use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use serde_json::{json, Map, Value};
use std::path::Path;
use tokio::fs;

/// Display information about S3 objects, buckets, or local files
pub async fn stat(
    client: &Client,
    path: &str,
    recursive: bool,
    checksum: Option<String>,
    json_output: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let path_type = parse_path(path)?;

    match path_type {
        PathType::S3 { bucket, key } => {
            if key.is_empty() {
                if recursive {
                    if json_output {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(
                                &stat_s3_recursive_json(client, &bucket, "").await?
                            )?
                        );
                        Ok(())
                    } else {
                        stat_s3_recursive(client, &bucket, "").await
                    }
                } else if json_output {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&stat_bucket_json(client, &bucket).await?)?
                    );
                    Ok(())
                } else {
                    stat_bucket(client, &bucket).await
                }
            } else if recursive {
                if json_output {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &stat_s3_recursive_json(client, &bucket, &key).await?
                        )?
                    );
                    Ok(())
                } else {
                    stat_s3_recursive(client, &bucket, &key).await
                }
            } else if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&stat_object_json(client, &bucket, &key).await?)?
                );
                Ok(())
            } else {
                stat_object(client, &bucket, &key).await
            }
        }
        PathType::Local(local_path) => {
            if recursive {
                if json_output {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &stat_local_recursive_json(&local_path, checksum).await?
                        )?
                    );
                    Ok(())
                } else {
                    stat_local_recursive(&local_path, checksum).await
                }
            } else if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&stat_local_json(&local_path, checksum).await?)?
                );
                Ok(())
            } else {
                stat_local(&local_path, checksum).await
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

            match s3_prefix_has_objects(client, bucket, "").await {
                Ok(Some(has_objects)) => {
                    if has_objects {
                        println!("Objects   : 1 (at least)");
                    } else {
                        println!("Objects   : 0 (empty)");
                    }
                }
                Ok(None) => {}
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

async fn stat_bucket_json(
    client: &Client,
    bucket: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut out = Map::new();
    out.insert("name".to_string(), json!(bucket));
    out.insert("type".to_string(), json!("s3 bucket"));

    client
        .head_bucket()
        .bucket(bucket)
        .send()
        .await
        .map_err(|e| {
            format!(
                "Bucket '{}' does not exist or is not accessible: {}",
                bucket, e
            )
        })?;
    out.insert("status".to_string(), json!("exists"));

    if let Ok(location) = client.get_bucket_location().bucket(bucket).send().await {
        out.insert(
            "region".to_string(),
            json!(location
                .location_constraint()
                .map(|c| c.as_str())
                .unwrap_or("us-east-1")),
        );
    }

    if let Ok(versioning) = client.get_bucket_versioning().bucket(bucket).send().await {
        if let Some(status) = versioning.status() {
            out.insert("versioning".to_string(), json!(status.as_str()));
        }
    }

    if client
        .get_bucket_encryption()
        .bucket(bucket)
        .send()
        .await
        .is_ok()
    {
        out.insert("encryption".to_string(), json!("Enabled"));
    }

    if let Ok(Some(has_objects)) = s3_prefix_has_objects(client, bucket, "").await {
        out.insert("objects".to_string(), json!(u8::from(has_objects)));
    }

    Ok(Value::Object(out))
}

/// Display S3 object information
async fn stat_object(
    client: &Client,
    bucket: &str,
    key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .checksum_mode(aws_sdk_s3::types::ChecksumMode::Enabled)
        .send()
        .await?;

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

    ObjectChecksums::from_head_object(&response).print_stat_lines();

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

async fn stat_object_json(
    client: &Client,
    bucket: &str,
    key: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let response = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .checksum_mode(aws_sdk_s3::types::ChecksumMode::Enabled)
        .send()
        .await?;

    let mut out = Map::new();
    out.insert(
        "name".to_string(),
        json!(format!("s3://{}/{}", bucket, key)),
    );
    out.insert("type".to_string(), json!("file"));
    if let Some(size) = response.content_length() {
        out.insert("size".to_string(), json!(size));
    }
    if let Some(last_modified) = response.last_modified() {
        out.insert("modified".to_string(), json!(last_modified.to_string()));
    }
    if let Some(etag) = response.e_tag() {
        out.insert("etag".to_string(), json!(etag));
    }
    if let Some(content_type) = response.content_type() {
        out.insert("content_type".to_string(), json!(content_type));
    }
    if let Some(storage_class) = response.storage_class() {
        out.insert("storage_class".to_string(), json!(storage_class.as_str()));
    }
    ObjectChecksums::from_head_object(&response).insert_stat_json(&mut out);
    if let Some(sse) = response.server_side_encryption() {
        out.insert("encryption".to_string(), json!(sse.as_str()));
    }
    if let Some(metadata) = response.metadata() {
        if !metadata.is_empty() {
            out.insert("metadata".to_string(), serde_json::to_value(metadata)?);
        }
    }
    if let Some(cache_control) = response.cache_control() {
        out.insert("cache_control".to_string(), json!(cache_control));
    }
    if let Some(expires) = response.expires_string() {
        out.insert("expires".to_string(), json!(expires));
    }
    Ok(Value::Object(out))
}

/// Display local filesystem information (S3-compatible format)
async fn stat_local(
    path: &str,
    checksum: Option<String>,
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
        if let Ok(etag) = compute_hash_for_local_path(path_obj, HashAlgorithm::Md5).await {
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

        if let Ok(Some(result)) = calculate_requested_checksum(path_obj, checksum.as_deref()).await
        {
            println!("{:<10}: {}", result.display_label, result.value);
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

async fn stat_local_json(
    path: &str,
    checksum: Option<String>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let normalized_path = path.trim_end_matches('/');
    let path_obj = Path::new(normalized_path);

    if !path_obj.exists() {
        return Err(format!("Path '{}' does not exist", normalized_path).into());
    }

    let metadata = fs::metadata(path_obj).await?;
    let mut out = Map::new();
    out.insert("name".to_string(), json!(normalized_path));

    let file_type = if metadata.is_dir() {
        "directory"
    } else if metadata.is_symlink() {
        "symbolic link"
    } else {
        "file"
    };
    out.insert("type".to_string(), json!(file_type));
    out.insert("size".to_string(), json!(metadata.len()));

    if let Ok(modified) = metadata.modified() {
        if let Ok(datetime) = modified.duration_since(std::time::UNIX_EPOCH) {
            let secs = datetime.as_secs();
            let dt = chrono::DateTime::from_timestamp(secs as i64, 0)
                .unwrap_or(chrono::DateTime::UNIX_EPOCH);
            out.insert(
                "modified".to_string(),
                json!(dt.format("%Y-%m-%d %H:%M:%S %Z").to_string()),
            );
        }
    }

    if metadata.is_file() {
        if let Ok(etag) = compute_hash_for_local_path(path_obj, HashAlgorithm::Md5).await {
            out.insert("etag".to_string(), json!(etag));
        }
        out.insert(
            "content_type".to_string(),
            json!(detect_content_type(path_obj)),
        );
        if checksum.is_some() {
            insert_requested_checksum(&mut out, path_obj, checksum).await?;
        }
    }

    out.insert("storage".to_string(), json!("local"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode();
        out.insert("mode".to_string(), json!(format!("{:o}", mode & 0o777)));
        out.insert("uid".to_string(), json!(metadata.uid()));
        out.insert("gid".to_string(), json!(metadata.gid()));
        out.insert("inode".to_string(), json!(metadata.ino()));
        out.insert("links".to_string(), json!(metadata.nlink()));
    }

    Ok(Value::Object(out))
}

fn detect_content_type(path_obj: &Path) -> &'static str {
    match path_obj.extension().and_then(|e| e.to_str()) {
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
    }
}

async fn insert_requested_checksum(
    out: &mut Map<String, Value>,
    path_obj: &Path,
    checksum: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(result) = calculate_requested_checksum(path_obj, checksum.as_deref()).await? {
        out.insert(result.json_key.to_string(), json!(result.value));
    }
    Ok(())
}

struct LocalChecksumResult {
    display_label: &'static str,
    json_key: &'static str,
    value: String,
}

async fn calculate_requested_checksum(
    path_obj: &Path,
    checksum: Option<&str>,
) -> Result<Option<LocalChecksumResult>, Box<dyn std::error::Error>> {
    let Some(checksum) = checksum else {
        return Ok(None);
    };

    let result = match checksum.to_uppercase().as_str() {
        "CRC32" => LocalChecksumResult {
            display_label: "CRC32",
            json_key: "crc32",
            value: compute_hash_for_local_path(path_obj, HashAlgorithm::Crc32).await?,
        },
        "CRC32C" => LocalChecksumResult {
            display_label: "CRC32C",
            json_key: "crc32c",
            value: compute_hash_for_local_path(path_obj, HashAlgorithm::Crc32c).await?,
        },
        "SHA1" => LocalChecksumResult {
            display_label: "SHA1",
            json_key: "sha1",
            value: compute_hash_for_local_path(path_obj, HashAlgorithm::Sha1).await?,
        },
        _ => LocalChecksumResult {
            display_label: "SHA256",
            json_key: "sha256",
            value: compute_hash_for_local_path(path_obj, HashAlgorithm::Sha256).await?,
        },
    };

    Ok(Some(result))
}

/// Stat local files recursively
async fn stat_local_recursive(
    path: &str,
    checksum: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path_obj = Path::new(path);

    if !path_obj.exists() {
        return Err(format!("Path '{}' does not exist", path).into());
    }

    if !path_obj.is_dir() {
        // Single file
        return stat_local(path, checksum).await;
    }

    for entry in walk_local_files(path)? {
        stat_local(
            entry
                .path
                .to_str()
                .ok_or_else(|| format!("path contains invalid UTF-8: {}", entry.path.display()))?,
            checksum.clone(),
        )
        .await?;
        println!(); // Blank line between entries
    }

    Ok(())
}

async fn stat_local_recursive_json(
    path: &str,
    checksum: Option<String>,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let path_obj = Path::new(path);

    if !path_obj.exists() {
        return Err(format!("Path '{}' does not exist", path).into());
    }

    if !path_obj.is_dir() {
        return Ok(vec![stat_local_json(path, checksum).await?]);
    }

    let mut entries = Vec::new();
    for entry in walk_local_files(path)? {
        entries.push(
            stat_local_json(
                entry.path.to_str().ok_or_else(|| {
                    format!("path contains invalid UTF-8: {}", entry.path.display())
                })?,
                checksum.clone(),
            )
            .await?,
        );
    }
    Ok(entries)
}

/// Stat S3 objects recursively
async fn stat_s3_recursive(
    client: &Client,
    bucket: &str,
    prefix: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for key in list_s3_keys(client, bucket, prefix).await? {
        stat_object(client, bucket, &key).await?;
        println!(); // Blank line between entries
    }

    Ok(())
}

async fn stat_s3_recursive_json(
    client: &Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut entries = Vec::new();

    for key in list_s3_keys(client, bucket, prefix).await? {
        entries.push(stat_object_json(client, bucket, &key).await?);
    }

    Ok(entries)
}

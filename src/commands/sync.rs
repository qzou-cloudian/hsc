use crate::filters::FileFilter;
use crate::path_utils::{join_s3_key, parse_path, PathType};
use aws_sdk_s3::Client;
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;
use walkdir::WalkDir;

/// Synchronize directories (copy only changed/new files)
pub async fn sync(
    client: &Client,
    source: &str,
    dest: &str,
    include: Vec<String>,
    exclude: Vec<String>,
    multipart_threshold: u64,
    multipart_chunksize: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let source_type = parse_path(source)?;
    let dest_type = parse_path(dest)?;
    let filter = FileFilter::new(include, exclude)?;

    match (&source_type, &dest_type) {
        (PathType::Local(src), PathType::S3 { bucket, key }) => {
            sync_local_to_s3(
                client,
                src,
                bucket,
                key,
                &filter,
                multipart_threshold,
                multipart_chunksize,
            )
            .await
        }
        (PathType::S3 { bucket, key }, PathType::Local(dst)) => {
            sync_s3_to_local(client, bucket, key, dst, &filter).await
        }
        (
            PathType::S3 {
                bucket: src_bucket,
                key: src_key,
            },
            PathType::S3 {
                bucket: dst_bucket,
                key: dst_key,
            },
        ) => sync_s3_to_s3(client, src_bucket, src_key, dst_bucket, dst_key, &filter).await,
        (PathType::Local(_), PathType::Local(_)) => {
            Err("Local to local sync not implemented. Use standard 'rsync' command.".into())
        }
    }
}

/// Sync local directory to S3
async fn sync_local_to_s3(
    client: &Client,
    local_dir: &str,
    bucket: &str,
    s3_prefix: &str,
    filter: &FileFilter,
    multipart_threshold: u64,
    multipart_chunksize: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::cp::upload_file;

    // Get existing S3 objects with their ETags/sizes
    let s3_objects = get_s3_objects(client, bucket, s3_prefix).await?;

    let base_path = Path::new(local_dir);
    let mut synced_count = 0;
    let mut skipped_count = 0;

    for entry in WalkDir::new(local_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();

        if path.is_file() {
            let relative_path = path
                .strip_prefix(base_path)
                .map_err(|e| format!("Path error: {}", e))?;
            let relative_str = relative_path.to_string_lossy().to_string();

            // Apply filters
            if !filter.matches(&relative_str) {
                continue;
            }

            let s3_key = join_s3_key(s3_prefix, &relative_str.replace("\\", "/"));

            // Check if file needs to be synced
            let needs_sync = match s3_objects.get(&s3_key) {
                Some(s3_size) => {
                    let local_size = fs::metadata(path).await?.len() as i64;
                    local_size != *s3_size
                }
                None => true, // File doesn't exist in S3
            };

            if needs_sync {
                upload_file(
                    client,
                    path.to_str().unwrap(),
                    bucket,
                    &s3_key,
                    None,
                    None,
                    multipart_threshold,
                    multipart_chunksize,
                )
                .await?;
                synced_count += 1;
            } else {
                skipped_count += 1;
            }
        }
    }

    println!(
        "\nSync complete: {} uploaded, {} skipped (unchanged)",
        synced_count, skipped_count
    );
    Ok(())
}

/// Sync S3 to local directory
async fn sync_s3_to_local(
    client: &Client,
    bucket: &str,
    prefix: &str,
    local_dir: &str,
    filter: &FileFilter,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::cp::download_file;

    let mut continuation_token: Option<String> = None;
    let mut synced_count = 0;
    let mut skipped_count = 0;

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
                // Apply filters
                if !filter.matches(key) {
                    continue;
                }

                let relative_key = if !prefix.is_empty() && key.starts_with(prefix) {
                    key[prefix.len()..].trim_start_matches('/')
                } else {
                    key
                };

                let local_path = Path::new(local_dir).join(relative_key);

                // Check if file needs to be synced
                let needs_sync = if local_path.exists() {
                    let local_size = fs::metadata(&local_path).await?.len() as i64;
                    let s3_size = obj.size().unwrap_or(0);
                    local_size != s3_size
                } else {
                    true
                };

                if needs_sync {
                    download_file(client, bucket, key, local_path.to_str().unwrap(), None).await?;
                    synced_count += 1;
                } else {
                    skipped_count += 1;
                }
            }
        }

        if response.is_truncated() == Some(true) {
            continuation_token = response.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    println!(
        "\nSync complete: {} downloaded, {} skipped (unchanged)",
        synced_count, skipped_count
    );
    Ok(())
}

/// Sync S3 to S3
async fn sync_s3_to_s3(
    client: &Client,
    src_bucket: &str,
    src_prefix: &str,
    dst_bucket: &str,
    dst_prefix: &str,
    filter: &FileFilter,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::cp::copy_s3_to_s3;

    // Get destination objects
    let dst_objects = get_s3_objects(client, dst_bucket, dst_prefix).await?;

    let mut continuation_token: Option<String> = None;
    let mut synced_count = 0;
    let mut skipped_count = 0;

    loop {
        let mut request = client.list_objects_v2().bucket(src_bucket);

        if !src_prefix.is_empty() {
            request = request.prefix(src_prefix);
        }

        if let Some(token) = continuation_token {
            request = request.continuation_token(token);
        }

        let response = request.send().await?;

        for obj in response.contents() {
            if let Some(key) = obj.key() {
                // Apply filters
                if !filter.matches(key) {
                    continue;
                }

                let relative_key = if !src_prefix.is_empty() && key.starts_with(src_prefix) {
                    key[src_prefix.len()..].trim_start_matches('/')
                } else {
                    key
                };

                let dst_key = join_s3_key(dst_prefix, relative_key);

                // Check if object needs to be synced
                let needs_sync = match dst_objects.get(&dst_key) {
                    Some(dst_size) => {
                        let src_size = obj.size().unwrap_or(0);
                        src_size != *dst_size
                    }
                    None => true,
                };

                if needs_sync {
                    copy_s3_to_s3(client, src_bucket, key, dst_bucket, &dst_key).await?;
                    synced_count += 1;
                } else {
                    skipped_count += 1;
                }
            }
        }

        if response.is_truncated() == Some(true) {
            continuation_token = response.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    println!(
        "\nSync complete: {} copied, {} skipped (unchanged)",
        synced_count, skipped_count
    );
    Ok(())
}

/// Get all objects in an S3 prefix as a map of key -> size
async fn get_s3_objects(
    client: &Client,
    bucket: &str,
    prefix: &str,
) -> Result<HashMap<String, i64>, Box<dyn std::error::Error>> {
    let mut objects = HashMap::new();
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
                let size = obj.size().unwrap_or(0);
                objects.insert(key.to_string(), size);
            }
        }

        if response.is_truncated() == Some(true) {
            continuation_token = response.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    Ok(objects)
}

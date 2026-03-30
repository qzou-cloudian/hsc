use crate::commands::cp::SseConfig;
use crate::filters::FileFilter;
use crate::path_utils::{join_s3_key, parse_path, PathType};
use aws_sdk_s3::types::{ChecksumAlgorithm, ChecksumMode};
use aws_sdk_s3::Client;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tokio::fs;
use walkdir::WalkDir;

#[cfg(feature = "rdma")]
use crate::rdma::RdmaProvider;
#[cfg(feature = "rdma")]
use std::sync::Arc;

/// Synchronize directories (copy only changed/new files)
#[allow(clippy::too_many_arguments)]
pub async fn sync(
    client: &Client,
    source: &str,
    dest: &str,
    include: Vec<String>,
    exclude: Vec<String>,
    checksum: Option<String>,
    delete: bool,
    sse: SseConfig,
    multipart_threshold: u64,
    multipart_chunksize: u64,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::cp::parse_checksum;
    let (checksum_mode, checksum_algorithm) = parse_checksum(checksum)?;

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
                checksum_mode,
                checksum_algorithm,
                delete,
                &sse,
                multipart_threshold,
                multipart_chunksize,
                #[cfg(feature = "rdma")]
                rdma,
            )
            .await
        }
        (PathType::S3 { bucket, key }, PathType::Local(dst)) => {
            sync_s3_to_local(
                client,
                bucket,
                key,
                dst,
                &filter,
                checksum_mode,
                delete,
                &sse,
                #[cfg(feature = "rdma")]
                rdma,
            )
            .await
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
        ) => {
            sync_s3_to_s3(
                client, src_bucket, src_key, dst_bucket, dst_key, &filter, delete, &sse,
            )
            .await
        }
        (PathType::Local(_), PathType::Local(_)) => {
            Err("Local to local sync not implemented. Use standard 'rsync' command.".into())
        }
    }
}

/// Sync local directory to S3
#[allow(clippy::too_many_arguments)]
async fn sync_local_to_s3(
    client: &Client,
    local_dir: &str,
    bucket: &str,
    s3_prefix: &str,
    filter: &FileFilter,
    checksum_mode: Option<ChecksumMode>,
    checksum_algorithm: Option<ChecksumAlgorithm>,
    delete: bool,
    sse: &SseConfig,
    multipart_threshold: u64,
    multipart_chunksize: u64,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::cp::upload_file;

    // Get existing S3 objects with their ETags/sizes
    let s3_objects = get_s3_objects(client, bucket, s3_prefix).await?;

    let base_path = Path::new(local_dir);
    let mut synced_count = 0;
    let mut skipped_count = 0;
    let mut local_keys: HashSet<String> = HashSet::new();

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
            local_keys.insert(s3_key.clone());

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
                    path.to_str().ok_or_else(|| {
                        format!("path contains invalid UTF-8: {}", path.display())
                    })?,
                    bucket,
                    &s3_key,
                    checksum_mode.clone(),
                    checksum_algorithm.clone(),
                    sse,
                    multipart_threshold,
                    multipart_chunksize,
                    #[cfg(feature = "rdma")]
                    rdma.as_ref().map(Arc::clone),
                )
                .await?;
                synced_count += 1;
            } else {
                skipped_count += 1;
            }
        }
    }

    if delete {
        let mut deleted_count = 0;
        for s3_key in s3_objects.keys() {
            if !local_keys.contains(s3_key) {
                client
                    .delete_object()
                    .bucket(bucket)
                    .key(s3_key)
                    .send()
                    .await?;
                println!("Deleted: s3://{}/{}", bucket, s3_key);
                deleted_count += 1;
            }
        }
        if deleted_count > 0 {
            println!("Deleted {} object(s) not present in source", deleted_count);
        }
    }

    println!(
        "\nSync complete: {} uploaded, {} skipped (unchanged)",
        synced_count, skipped_count
    );
    Ok(())
}

/// Sync S3 to local directory
#[allow(clippy::too_many_arguments)]
async fn sync_s3_to_local(
    client: &Client,
    bucket: &str,
    prefix: &str,
    local_dir: &str,
    filter: &FileFilter,
    checksum_mode: Option<ChecksumMode>,
    delete: bool,
    sse: &SseConfig,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::cp::download_file;

    let mut continuation_token: Option<String> = None;
    let mut synced_count = 0;
    let mut skipped_count = 0;
    let mut s3_relative_keys: HashSet<String> = HashSet::new();

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
                if !filter.matches(key) {
                    continue;
                }

                let relative_key = if !prefix.is_empty() && key.starts_with(prefix) {
                    key[prefix.len()..].trim_start_matches('/')
                } else {
                    key
                };

                s3_relative_keys.insert(relative_key.to_string());

                let local_path = Path::new(local_dir).join(relative_key);

                let needs_sync = if local_path.exists() {
                    let local_size = fs::metadata(&local_path).await?.len() as i64;
                    let s3_size = obj.size().unwrap_or(0);
                    local_size != s3_size
                } else {
                    true
                };

                if needs_sync {
                    download_file(
                        client,
                        bucket,
                        key,
                        local_path.to_str().ok_or_else(|| {
                            format!("path contains invalid UTF-8: {}", local_path.display())
                        })?,
                        checksum_mode.clone(),
                        sse,
                        #[cfg(feature = "rdma")]
                        rdma.as_ref().map(Arc::clone),
                    )
                    .await?;
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

    if delete {
        let mut deleted_count = 0;
        for entry in WalkDir::new(local_dir).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let relative = match path.strip_prefix(Path::new(local_dir)) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if !filter.matches(&relative) {
                continue;
            }
            if !s3_relative_keys.contains(&relative) {
                fs::remove_file(path).await?;
                println!("Deleted: {}", path.display());
                deleted_count += 1;
            }
        }
        if deleted_count > 0 {
            println!(
                "Deleted {} local file(s) not present in source",
                deleted_count
            );
        }
    }

    println!(
        "\nSync complete: {} downloaded, {} skipped (unchanged)",
        synced_count, skipped_count
    );
    Ok(())
}

/// Sync S3 to S3
#[allow(clippy::too_many_arguments)]
async fn sync_s3_to_s3(
    client: &Client,
    src_bucket: &str,
    src_prefix: &str,
    dst_bucket: &str,
    dst_prefix: &str,
    filter: &FileFilter,
    delete: bool,
    sse: &SseConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::cp::copy_s3_to_s3;

    // Get destination objects
    let dst_objects = get_s3_objects(client, dst_bucket, dst_prefix).await?;

    let mut continuation_token: Option<String> = None;
    let mut synced_count = 0;
    let mut skipped_count = 0;
    let mut expected_dst_keys: HashSet<String> = HashSet::new();

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
                expected_dst_keys.insert(dst_key.clone());

                // Check if object needs to be synced
                let needs_sync = match dst_objects.get(&dst_key) {
                    Some(dst_size) => {
                        let src_size = obj.size().unwrap_or(0);
                        src_size != *dst_size
                    }
                    None => true,
                };

                if needs_sync {
                    copy_s3_to_s3(client, src_bucket, key, dst_bucket, &dst_key, sse).await?;
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

    if delete {
        let mut deleted_count = 0;
        for dst_key in dst_objects.keys() {
            if !expected_dst_keys.contains(dst_key) {
                client
                    .delete_object()
                    .bucket(dst_bucket)
                    .key(dst_key)
                    .send()
                    .await?;
                println!("Deleted: s3://{}/{}", dst_bucket, dst_key);
                deleted_count += 1;
            }
        }
        if deleted_count > 0 {
            println!("Deleted {} object(s) not present in source", deleted_count);
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

use crate::commands::cp::{DownloadOptions, UploadOptions};
use crate::commands::listing::{list_s3_objects, walk_local_files};
use crate::commands::transfer::{MultipartConfig, SseConfig};
use crate::filters::FileFilter;
use crate::path_utils::{join_s3_key, parse_path, PathType};
use aws_sdk_s3::types::{ChecksumAlgorithm, ChecksumMode};
use aws_sdk_s3::Client;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tokio::fs;

#[cfg(feature = "rdma")]
use crate::rdma::RdmaClientProvider;
#[cfg(feature = "rdma")]
use std::sync::Arc;

/// Synchronize directories (copy only changed/new files)
pub struct SyncOptions {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub checksum: Option<String>,
    pub delete: bool,
    pub sse: SseConfig,
    pub multipart: MultipartConfig,
    #[cfg(feature = "rdma")]
    pub rdma: Option<Arc<dyn RdmaClientProvider>>,
}

struct SyncChecksumOptions {
    mode: Option<ChecksumMode>,
    algorithm: Option<ChecksumAlgorithm>,
}

pub async fn sync(
    client: &Client,
    source: &str,
    dest: &str,
    opts: SyncOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::transfer::parse_checksum;
    let (checksum_mode, checksum_algorithm) = parse_checksum(opts.checksum.clone())?;

    let source_type = parse_path(source)?;
    let dest_type = parse_path(dest)?;
    let filter = FileFilter::new(opts.include.clone(), opts.exclude.clone())?;

    match (&source_type, &dest_type) {
        (PathType::Local(src), PathType::S3 { bucket, key }) => {
            sync_local_to_s3(
                client,
                src,
                bucket,
                key,
                &filter,
                &SyncChecksumOptions {
                    mode: checksum_mode,
                    algorithm: checksum_algorithm,
                },
                &opts,
            )
            .await
        }
        (PathType::S3 { bucket, key }, PathType::Local(dst)) => {
            sync_s3_to_local(client, bucket, key, dst, &filter, checksum_mode, &opts).await
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
                client, src_bucket, src_key, dst_bucket, dst_key, &filter, &opts,
            )
            .await
        }
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
    checksum: &SyncChecksumOptions,
    opts: &SyncOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::cp::upload_file;

    // Get existing S3 objects with their ETags/sizes
    let s3_objects = get_s3_objects(client, bucket, s3_prefix).await?;

    let mut synced_count = 0;
    let mut skipped_count = 0;
    let mut local_keys: HashSet<String> = HashSet::new();

    for entry in walk_local_files(local_dir)? {
        if !filter.matches(&entry.relative) {
            continue;
        }

        let s3_key = join_s3_key(s3_prefix, &entry.relative_unix);
        local_keys.insert(s3_key.clone());

        let needs_sync = match s3_objects.get(&s3_key) {
            Some(s3_size) => {
                let local_size = fs::metadata(&entry.path).await?.len() as i64;
                local_size != *s3_size
            }
            None => true,
        };

        if needs_sync {
            upload_file(
                client,
                entry.path.to_str().ok_or_else(|| {
                    format!("path contains invalid UTF-8: {}", entry.path.display())
                })?,
                bucket,
                &s3_key,
                UploadOptions {
                    checksum_mode: checksum.mode.clone(),
                    checksum_algorithm: checksum.algorithm.clone(),
                    sse: &opts.sse,
                    multipart: opts.multipart,
                    #[cfg(feature = "rdma")]
                    rdma: opts.rdma.as_ref().map(Arc::clone),
                },
            )
            .await?;
            synced_count += 1;
        } else {
            skipped_count += 1;
        }
    }

    if opts.delete {
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
async fn sync_s3_to_local(
    client: &Client,
    bucket: &str,
    prefix: &str,
    local_dir: &str,
    filter: &FileFilter,
    checksum_mode: Option<ChecksumMode>,
    opts: &SyncOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::cp::download_file;

    let mut synced_count = 0;
    let mut skipped_count = 0;
    let mut s3_relative_keys: HashSet<String> = HashSet::new();

    for obj in list_s3_objects(client, bucket, prefix).await? {
        if !filter.matches(&obj.key) {
            continue;
        }

        let relative_key = obj.relative_key(prefix);
        s3_relative_keys.insert(relative_key.to_string());

        let local_path = Path::new(local_dir).join(relative_key);

        let needs_sync = if local_path.exists() {
            let local_size = fs::metadata(&local_path).await?.len() as i64;
            local_size != obj.size
        } else {
            true
        };

        if needs_sync {
            download_file(
                client,
                bucket,
                &obj.key,
                local_path.to_str().ok_or_else(|| {
                    format!("path contains invalid UTF-8: {}", local_path.display())
                })?,
                DownloadOptions {
                    checksum_mode: checksum_mode.clone(),
                    sse: &opts.sse,
                    #[cfg(feature = "rdma")]
                    rdma: opts.rdma.as_ref().map(Arc::clone),
                },
            )
            .await?;
            synced_count += 1;
        } else {
            skipped_count += 1;
        }
    }

    if opts.delete {
        let mut deleted_count = 0;
        for entry in walk_local_files(local_dir)? {
            if !filter.matches(&entry.relative_unix) {
                continue;
            }
            if !s3_relative_keys.contains(&entry.relative_unix) {
                fs::remove_file(&entry.path).await?;
                println!("Deleted: {}", entry.path.display());
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
async fn sync_s3_to_s3(
    client: &Client,
    src_bucket: &str,
    src_prefix: &str,
    dst_bucket: &str,
    dst_prefix: &str,
    filter: &FileFilter,
    opts: &SyncOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::commands::cp::copy_s3_to_s3;

    // Get destination objects
    let dst_objects = get_s3_objects(client, dst_bucket, dst_prefix).await?;

    let mut synced_count = 0;
    let mut skipped_count = 0;
    let mut expected_dst_keys: HashSet<String> = HashSet::new();

    for obj in list_s3_objects(client, src_bucket, src_prefix).await? {
        if !filter.matches(&obj.key) {
            continue;
        }

        let dst_key = join_s3_key(dst_prefix, obj.relative_key(src_prefix));
        expected_dst_keys.insert(dst_key.clone());

        let needs_sync = match dst_objects.get(&dst_key) {
            Some(dst_size) => obj.size != *dst_size,
            None => true,
        };

        if needs_sync {
            copy_s3_to_s3(
                client, src_bucket, &obj.key, dst_bucket, &dst_key, &opts.sse,
            )
            .await?;
            synced_count += 1;
        } else {
            skipped_count += 1;
        }
    }

    if opts.delete {
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
    Ok(list_s3_objects(client, bucket, prefix)
        .await?
        .into_iter()
        .map(|obj| (obj.key, obj.size))
        .collect())
}

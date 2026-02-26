use crate::filters::FileFilter;
use crate::path_utils::{join_s3_key, parse_path, PathType};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{ChecksumAlgorithm, ChecksumMode, CompletedMultipartUpload, CompletedPart};
use aws_sdk_s3::Client;
use std::path::Path;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use walkdir::WalkDir;

/// Copy files between local and S3
pub async fn copy(
    client: &Client,
    source: &str,
    dest: &str,
    recursive: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    checksum_mode: Option<String>,
    checksum_algorithm: Option<String>,
    multipart_threshold: u64,
    multipart_chunksize: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let source_type = parse_path(source)?;
    let dest_type = parse_path(dest)?;

    // Parse checksum options (only for single object operations)
    let checksum_opts = if !recursive {
        parse_checksum_options(checksum_mode, checksum_algorithm)?
    } else {
        if checksum_mode.is_some() || checksum_algorithm.is_some() {
            eprintln!("Warning: Checksum options are ignored for recursive operations");
        }
        (None, None)
    };

    if recursive {
        let filter = FileFilter::new(include, exclude)?;
        copy_recursive(
            client,
            source_type,
            dest_type,
            &filter,
            multipart_threshold,
            multipart_chunksize,
        )
        .await
    } else {
        copy_single(
            client,
            source_type,
            dest_type,
            checksum_opts.0,
            checksum_opts.1,
            multipart_threshold,
            multipart_chunksize,
        )
        .await
    }
}

/// Parse checksum options
fn parse_checksum_options(
    mode: Option<String>,
    algorithm: Option<String>,
) -> Result<(Option<ChecksumMode>, Option<ChecksumAlgorithm>), String> {
    let checksum_mode = if let Some(m) = mode {
        match m.to_uppercase().as_str() {
            "ENABLED" => Some(ChecksumMode::Enabled),
            _ => return Err(format!("Invalid checksum mode: {}. Use ENABLED", m)),
        }
    } else {
        None
    };

    let checksum_algo = if let Some(a) = algorithm {
        match a.to_uppercase().as_str() {
            "CRC32" => Some(ChecksumAlgorithm::Crc32),
            "CRC32C" => Some(ChecksumAlgorithm::Crc32C),
            "SHA1" => Some(ChecksumAlgorithm::Sha1),
            "SHA256" => Some(ChecksumAlgorithm::Sha256),
            _ => {
                return Err(format!(
                    "Invalid checksum algorithm: {}. Use CRC32, CRC32C, SHA1, or SHA256",
                    a
                ))
            }
        }
    } else {
        None
    };

    Ok((checksum_mode, checksum_algo))
}

/// Copy a single file
async fn copy_single(
    client: &Client,
    source: PathType,
    dest: PathType,
    checksum_mode: Option<ChecksumMode>,
    checksum_algorithm: Option<ChecksumAlgorithm>,
    multipart_threshold: u64,
    multipart_chunksize: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    match (&source, &dest) {
        (PathType::Local(src), PathType::S3 { bucket, key }) => {
            // Local to S3
            upload_file(
                client,
                src,
                bucket,
                key,
                checksum_mode,
                checksum_algorithm,
                multipart_threshold,
                multipart_chunksize,
            )
            .await
        }
        (PathType::S3 { bucket, key }, PathType::Local(dst)) => {
            // S3 to local
            download_file(client, bucket, key, dst, checksum_mode).await
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
            // S3 to S3
            copy_s3_to_s3(client, src_bucket, src_key, dst_bucket, dst_key).await
        }
        (PathType::Local(src), PathType::Local(dst)) => {
            // Local to local
            fs::copy(src, dst).await?;
            println!("Copied: {} -> {}", src, dst);
            Ok(())
        }
    }
}

/// Upload a file to S3
pub async fn upload_file(
    client: &Client,
    local_path: &str,
    bucket: &str,
    key: &str,
    checksum_mode: Option<ChecksumMode>,
    checksum_algorithm: Option<ChecksumAlgorithm>,
    multipart_threshold: u64,
    multipart_chunksize: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check file size
    let metadata = fs::metadata(local_path).await?;
    let file_size = metadata.len();

    if file_size >= multipart_threshold {
        // Use multipart upload
        upload_file_multipart(
            client,
            local_path,
            bucket,
            key,
            file_size,
            multipart_chunksize,
        )
        .await
    } else {
        // Use regular put_object
        let body = ByteStream::from_path(Path::new(local_path)).await?;

        let mut request = client.put_object().bucket(bucket).key(key).body(body);

        if checksum_mode.is_some() {
            request =
                request.checksum_algorithm(checksum_algorithm.unwrap_or(ChecksumAlgorithm::Crc32));
        }

        request.send().await?;

        println!("Uploaded: {} -> s3://{}/{}", local_path, bucket, key);
        Ok(())
    }
}

/// Upload a file to S3 using multipart upload
async fn upload_file_multipart(
    client: &Client,
    local_path: &str,
    bucket: &str,
    key: &str,
    file_size: u64,
    chunk_size: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "Using multipart upload for {} ({} bytes, {} bytes per part)",
        local_path, file_size, chunk_size
    );

    // Step 1: Create multipart upload
    let multipart_upload = client
        .create_multipart_upload()
        .bucket(bucket)
        .key(key)
        .send()
        .await?;

    let upload_id = multipart_upload
        .upload_id()
        .ok_or("Failed to get upload ID")?;

    // Step 2: Upload parts
    let mut parts = Vec::new();
    let mut file = fs::File::open(local_path).await?;
    let mut part_number = 1;
    let mut uploaded_bytes = 0u64;

    loop {
        let mut buffer = vec![0u8; chunk_size as usize];
        let mut bytes_read = 0;

        // Read chunk_size bytes
        while bytes_read < chunk_size as usize {
            let n = file.read(&mut buffer[bytes_read..]).await?;
            if n == 0 {
                break; // EOF
            }
            bytes_read += n;
        }

        if bytes_read == 0 {
            break; // No more data
        }

        // Trim buffer to actual size read
        buffer.truncate(bytes_read);

        // Upload this part
        let body = ByteStream::from(buffer);
        let upload_part_response = client
            .upload_part()
            .bucket(bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(part_number)
            .body(body)
            .send()
            .await?;

        let etag = upload_part_response
            .e_tag()
            .ok_or("Failed to get ETag for part")?
            .to_string();

        parts.push(
            CompletedPart::builder()
                .part_number(part_number)
                .e_tag(etag)
                .build(),
        );

        uploaded_bytes += bytes_read as u64;
        println!(
            "Uploaded part {}: {} / {} bytes ({:.1}%)",
            part_number,
            uploaded_bytes,
            file_size,
            (uploaded_bytes as f64 / file_size as f64) * 100.0
        );

        part_number += 1;

        if bytes_read < chunk_size as usize {
            break; // Last part
        }
    }

    // Step 3: Complete multipart upload
    let completed_upload = CompletedMultipartUpload::builder()
        .set_parts(Some(parts))
        .build();

    client
        .complete_multipart_upload()
        .bucket(bucket)
        .key(key)
        .upload_id(upload_id)
        .multipart_upload(completed_upload)
        .send()
        .await?;

    println!(
        "Multipart upload completed: {} -> s3://{}/{}",
        local_path, bucket, key
    );
    Ok(())
}

/// Download a file from S3
pub async fn download_file(
    client: &Client,
    bucket: &str,
    key: &str,
    local_path: &str,
    checksum_mode: Option<ChecksumMode>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut request = client.get_object().bucket(bucket).key(key);

    if let Some(mode) = checksum_mode {
        request = request.checksum_mode(mode);
    }

    let response = request.send().await?;

    // Create parent directories if needed
    if let Some(parent) = Path::new(local_path).parent() {
        fs::create_dir_all(parent).await?;
    }

    let mut file = fs::File::create(local_path).await?;
    let mut body = response.body;

    while let Some(chunk) = body.try_next().await? {
        file.write_all(&chunk).await?;
    }

    println!("Downloaded: s3://{}/{} -> {}", bucket, key, local_path);
    Ok(())
}

/// Copy object from S3 to S3
pub async fn copy_s3_to_s3(
    client: &Client,
    src_bucket: &str,
    src_key: &str,
    dst_bucket: &str,
    dst_key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let copy_source = format!("{}/{}", src_bucket, src_key);

    client
        .copy_object()
        .copy_source(&copy_source)
        .bucket(dst_bucket)
        .key(dst_key)
        .send()
        .await?;

    println!(
        "Copied: s3://{}/{} -> s3://{}/{}",
        src_bucket, src_key, dst_bucket, dst_key
    );
    Ok(())
}

/// Copy files recursively
async fn copy_recursive(
    client: &Client,
    source: PathType,
    dest: PathType,
    filter: &FileFilter,
    multipart_threshold: u64,
    multipart_chunksize: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    match (&source, &dest) {
        (PathType::Local(src), PathType::S3 { bucket, key }) => {
            // Local directory to S3
            upload_directory(
                client,
                src,
                bucket,
                key,
                filter,
                multipart_threshold,
                multipart_chunksize,
            )
            .await
        }
        (PathType::S3 { bucket, key }, PathType::Local(dst)) => {
            // S3 prefix to local directory
            download_directory(client, bucket, key, dst, filter).await
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
            // S3 to S3 recursive
            copy_s3_directory(client, src_bucket, src_key, dst_bucket, dst_key, filter).await
        }
        (PathType::Local(_), PathType::Local(_)) => Err(
            "Local to local recursive copy not implemented. Use standard 'cp -r' command.".into(),
        ),
    }
}

/// Upload a directory to S3
async fn upload_directory(
    client: &Client,
    local_dir: &str,
    bucket: &str,
    s3_prefix: &str,
    filter: &FileFilter,
    multipart_threshold: u64,
    multipart_chunksize: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let base_path = Path::new(local_dir);

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
        }
    }

    Ok(())
}

/// Download S3 prefix to local directory
async fn download_directory(
    client: &Client,
    bucket: &str,
    prefix: &str,
    local_dir: &str,
    filter: &FileFilter,
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
                download_file(client, bucket, key, local_path.to_str().unwrap(), None).await?;
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

/// Copy S3 directory to another S3 location
async fn copy_s3_directory(
    client: &Client,
    src_bucket: &str,
    src_prefix: &str,
    dst_bucket: &str,
    dst_prefix: &str,
    filter: &FileFilter,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut continuation_token: Option<String> = None;

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
                copy_s3_to_s3(client, src_bucket, key, dst_bucket, &dst_key).await?;
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

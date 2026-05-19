use crate::commands::listing::{list_s3_objects, walk_local_files};
use crate::commands::transfer::{
    apply_destination_sse_customer_to_copy_object, apply_put_checksum_to_put_object,
    apply_put_checksum_to_upload_part, apply_server_side_encryption_to_copy_object,
    apply_server_side_encryption_to_create_multipart, apply_server_side_encryption_to_put_object,
    apply_source_sse_customer_to_copy_object, apply_sse_customer_to_create_multipart,
    apply_sse_customer_to_get_object, apply_sse_customer_to_put_object,
    apply_sse_customer_to_upload_part, completed_part, compute_put_checksum, parse_checksum,
    stream_body_to_writer, MultipartConfig, SseConfig,
};
use crate::filters::FileFilter;
use crate::path_utils::{join_s3_key, parse_path, PathType};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{ChecksumAlgorithm, ChecksumMode, CompletedMultipartUpload};
use aws_sdk_s3::Client;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncReadExt;

#[cfg(feature = "rdma")]
use crate::commands::transfer::{
    bind_rdma_channel, prepare_rdma_get_multi, prepare_rdma_put_multi, prepare_rdma_put_single,
    send_get_with_optional_rdma_to_writer,
};
#[cfg(feature = "rdma")]
use crate::rdma::{RdmaClientProvider, RdmaInterceptor};

#[cfg(feature = "rdma")]
use std::sync::{atomic::AtomicBool, Arc};

pub struct CopyOptions {
    pub recursive: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub checksum: Option<String>,
    pub sse: SseConfig,
    pub multipart: MultipartConfig,
    #[cfg(feature = "rdma")]
    pub rdma: Option<Arc<dyn RdmaClientProvider>>,
}

pub(crate) struct UploadOptions<'a> {
    pub checksum_mode: Option<ChecksumMode>,
    pub checksum_algorithm: Option<ChecksumAlgorithm>,
    pub sse: &'a SseConfig,
    pub multipart: MultipartConfig,
    #[cfg(feature = "rdma")]
    pub rdma: Option<Arc<dyn RdmaClientProvider>>,
}

pub(crate) struct DownloadOptions<'a> {
    pub checksum_mode: Option<ChecksumMode>,
    pub sse: &'a SseConfig,
    #[cfg(feature = "rdma")]
    pub rdma: Option<Arc<dyn RdmaClientProvider>>,
}

/// Copy files between local and S3
pub async fn copy(
    client: &Client,
    source: &str,
    dest: &str,
    opts: CopyOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let source_type = parse_path(source)?;
    let dest_type = parse_path(dest)?;

    // Parse checksum options (only for single object operations)
    let checksum_opts = if !opts.recursive {
        parse_checksum(opts.checksum.clone())?
    } else {
        if opts.checksum.is_some() {
            eprintln!("Warning: Checksum option is ignored for recursive operations");
        }
        (None, None)
    };

    if opts.recursive {
        let filter = FileFilter::new(opts.include.clone(), opts.exclude.clone())?;
        copy_recursive(client, source_type, dest_type, &filter, &opts).await
    } else {
        copy_single(
            client,
            source_type,
            dest_type,
            &UploadOptions {
                checksum_mode: checksum_opts.0,
                checksum_algorithm: checksum_opts.1,
                sse: &opts.sse,
                multipart: opts.multipart,
                #[cfg(feature = "rdma")]
                rdma: opts.rdma,
            },
        )
        .await
    }
}

/// Copy a single file
async fn copy_single(
    client: &Client,
    source: PathType,
    dest: PathType,
    opts: &UploadOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    match (&source, &dest) {
        (PathType::Local(src), PathType::S3 { bucket, key }) => {
            // Local to S3
            upload_file(
                client,
                src,
                bucket,
                key,
                UploadOptions {
                    checksum_mode: opts.checksum_mode.clone(),
                    checksum_algorithm: opts.checksum_algorithm.clone(),
                    sse: opts.sse,
                    multipart: opts.multipart,
                    #[cfg(feature = "rdma")]
                    rdma: opts.rdma.as_ref().map(Arc::clone),
                },
            )
            .await
        }
        (PathType::S3 { bucket, key }, PathType::Local(dst)) => {
            // S3 to local
            download_file(
                client,
                bucket,
                key,
                dst,
                DownloadOptions {
                    checksum_mode: opts.checksum_mode.clone(),
                    sse: opts.sse,
                    #[cfg(feature = "rdma")]
                    rdma: opts.rdma.as_ref().map(Arc::clone),
                },
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
            // S3 to S3
            copy_s3_to_s3(client, src_bucket, src_key, dst_bucket, dst_key, opts.sse).await
        }
        (PathType::Local(src), PathType::Local(dst)) => {
            // Local to local
            fs::copy(src, dst).await?;
            println!("Copied: {} -> {}", src, dst);
            Ok(())
        }
    }
}

/// Upload a single multipart part via a plain HTTP body (non-RDMA path).
///
/// Computes the checksum over `data` before sending so that the value is set
/// as a regular request header rather than an aws-chunked trailer.  Trailing
/// checksums require the server to support `aws-chunked` transfer encoding,
/// which many S3-compatible servers (including Cloudian) do not.
///
/// Returns the SDK response together with the computed checksum value.  The
/// checksum value is kept as a fallback for `CompleteMultipartUpload`: some
/// servers (e.g. RDMA paths with an empty HTTP body) do not echo the
/// per-part checksum back in the `UploadPart` response.
struct UploadPartHttpOptions<'a> {
    bucket: &'a str,
    key: &'a str,
    upload_id: &'a str,
    part_number: i32,
    checksum_mode: Option<&'a ChecksumMode>,
    checksum_algorithm: Option<&'a ChecksumAlgorithm>,
    sse: &'a SseConfig,
}

async fn upload_part_http(
    client: &Client,
    data: Vec<u8>,
    opts: UploadPartHttpOptions<'_>,
) -> Result<
    (
        aws_sdk_s3::operation::upload_part::UploadPartOutput,
        Option<String>,
    ),
    Box<dyn std::error::Error>,
> {
    let (algo_name, cksum_val) =
        compute_put_checksum(&data, opts.checksum_mode, opts.checksum_algorithm);
    let mut req = client
        .upload_part()
        .bucket(opts.bucket)
        .key(opts.key)
        .upload_id(opts.upload_id)
        .part_number(opts.part_number)
        .body(ByteStream::from(data));
    req = apply_put_checksum_to_upload_part(req, algo_name.as_deref(), cksum_val.as_deref());
    req = apply_sse_customer_to_upload_part(req, opts.sse.destination_customer_headers()?);
    Ok((req.send().await?, cksum_val))
}

/// Upload a file to S3
pub async fn upload_file(
    client: &Client,
    local_path: &str,
    bucket: &str,
    key: &str,
    opts: UploadOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check file size
    let metadata = fs::metadata(local_path).await?;
    let file_size = metadata.len();

    if file_size >= opts.multipart.threshold {
        // Use multipart upload
        upload_file_multipart(client, local_path, bucket, key, file_size, &opts).await
    } else {
        // Single PUT — use RDMA when a provider is supplied.
        #[cfg(feature = "rdma")]
        if let Some(ref provider) = opts.rdma {
            let data = tokio::fs::read(local_path).await?;
            let size = data.len();
            let s3_key = format!("{bucket}/{key}");
            let maybe_rdma =
                bind_rdma_channel(provider, size, s3_key.as_bytes()).and_then(|channel| {
                    let buf =
                        unsafe { std::slice::from_raw_parts_mut(channel.ptr(), channel.size()) };
                    buf[..size].copy_from_slice(&data[..size]);
                    prepare_rdma_put_multi(&channel, size)
                });
            // For RDMA transfers the data travels via RDMA, not the HTTP body.
            // Use an empty body so content-length is 0 and x-amz-content-sha256
            // reflects SHA256 of the empty string, as the server expects.
            let rdma_body = ByteStream::from_static(b"");
            if let Some(prepared) = maybe_rdma {
                let rdma_confirmed = Arc::new(AtomicBool::new(false));
                let (cksum_alg, cksum_val) = compute_put_checksum(
                    &data,
                    opts.checksum_mode.as_ref(),
                    opts.checksum_algorithm.as_ref(),
                );
                let interceptor = RdmaInterceptor::new_put(
                    prepared.channel,
                    prepared.token,
                    prepared.handles,
                    size,
                    rdma_confirmed,
                    false,
                    cksum_alg,
                    cksum_val,
                );
                client
                    .put_object()
                    .bucket(bucket)
                    .key(key)
                    .body(rdma_body)
                    .customize()
                    .interceptor(interceptor)
                    .send()
                    .await?;
                // prepared.channel is dropped here and deregisters memory.
                println!("Uploaded: {local_path} -> s3://{bucket}/{key}");
                return Ok(());
            }
            // RDMA bind or prepare failed; fall through to standard HTTP upload below.
        }

        let body;
        let mut request;
        if opts.checksum_mode.is_some() {
            // Buffer the file so we can pre-compute the checksum as a plain
            // request header.  Using checksum_algorithm() on a streaming body
            // causes the SDK to use aws-chunked/trailing-checksum encoding,
            // which many S3-compatible servers do not support.
            let bytes = tokio::fs::read(local_path).await?;
            let (algo_name, cksum_val) = compute_put_checksum(
                &bytes,
                opts.checksum_mode.as_ref(),
                opts.checksum_algorithm.as_ref(),
            );
            body = ByteStream::from(bytes);
            request = client.put_object().bucket(bucket).key(key).body(body);
            request = apply_put_checksum_to_put_object(request, algo_name.as_deref(), cksum_val);
        } else {
            body = ByteStream::from_path(Path::new(local_path)).await?;
            request = client.put_object().bucket(bucket).key(key).body(body);
        }
        request = apply_server_side_encryption_to_put_object(
            request,
            opts.sse.sse.as_deref(),
            opts.sse.sse_kms_key_id.as_deref(),
        );
        request =
            apply_sse_customer_to_put_object(request, opts.sse.destination_customer_headers()?);
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
    opts: &UploadOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "Using multipart upload for {} ({} bytes, {} bytes per part)",
        local_path, file_size, opts.multipart.chunksize
    );

    let effective_algo = opts
        .checksum_algorithm
        .clone()
        .unwrap_or(ChecksumAlgorithm::Crc32);

    // Step 1: Create multipart upload
    let mut create_req = client.create_multipart_upload().bucket(bucket).key(key);
    if opts.checksum_mode.is_some() {
        create_req = create_req.checksum_algorithm(effective_algo.clone());
    }
    create_req = apply_server_side_encryption_to_create_multipart(
        create_req,
        opts.sse.sse.as_deref(),
        opts.sse.sse_kms_key_id.as_deref(),
    );
    create_req = apply_sse_customer_to_create_multipart(
        create_req,
        opts.sse.destination_customer_headers()?,
    );
    let multipart_upload = create_req.send().await?;

    let upload_id = multipart_upload
        .upload_id()
        .ok_or("Failed to get upload ID")?;

    // Step 2: Upload parts
    let mut parts = Vec::new();
    let mut file = fs::File::open(local_path).await?;
    let mut part_number = 1;
    let mut uploaded_bytes = 0u64;

    loop {
        // Read the next chunk.  Both RDMA and plain-HTTP paths read into a
        // local Vec first; the RDMA path then copies into the provider-owned
        // channel buffer via channel.ptr().
        let mut chunk_buf = vec![0u8; opts.multipart.chunksize as usize];
        let mut n_read = 0usize;
        while n_read < opts.multipart.chunksize as usize {
            let n = file.read(&mut chunk_buf[n_read..]).await?;
            if n == 0 {
                break;
            }
            n_read += n;
        }
        if n_read == 0 {
            break;
        }
        chunk_buf.truncate(n_read);
        let bytes_read = n_read;

        // Upload this part — with RDMA interceptor when a provider is present.
        // part_cksum_val holds the locally computed checksum for CompleteMultipartUpload:
        // some servers (e.g. empty-body RDMA path) don't echo the checksum in the
        // UploadPart response, so we keep our own copy as a fallback.
        let part_cksum_val: Option<String>;
        #[cfg(feature = "rdma")]
        let upload_part_response = {
            if let Some(ref provider) = opts.rdma {
                let s3_key = format!("{bucket}/{key}");
                let maybe_rdma = bind_rdma_channel(provider, bytes_read, s3_key.as_bytes())
                    .and_then(|channel| {
                        let buf = unsafe {
                            std::slice::from_raw_parts_mut(channel.ptr(), channel.size())
                        };
                        buf[..bytes_read].copy_from_slice(&chunk_buf[..bytes_read]);
                        prepare_rdma_put_single(&channel, bytes_read, "for part ")
                    });

                let resp = if let Some(prepared) = maybe_rdma {
                    let rdma_confirmed = Arc::new(AtomicBool::new(false));
                    let (cksum_alg, cksum_val) = compute_put_checksum(
                        &chunk_buf[..bytes_read],
                        opts.checksum_mode.as_ref(),
                        opts.checksum_algorithm.as_ref(),
                    );
                    part_cksum_val = cksum_val.clone();
                    let interceptor = RdmaInterceptor::new_put(
                        prepared.channel,
                        prepared.token,
                        prepared.handles,
                        bytes_read,
                        rdma_confirmed,
                        false,
                        cksum_alg,
                        cksum_val,
                    );
                    client
                        .upload_part()
                        .bucket(bucket)
                        .key(key)
                        .upload_id(upload_id)
                        .part_number(part_number)
                        .body(ByteStream::from_static(b""))
                        .customize()
                        .interceptor(interceptor)
                        .send()
                        .await?
                } else {
                    // RDMA bind or prepare_put unavailable — fall back to plain HTTP body.
                    let (resp, cksum) = upload_part_http(
                        client,
                        chunk_buf,
                        UploadPartHttpOptions {
                            bucket,
                            key,
                            upload_id,
                            part_number,
                            checksum_mode: opts.checksum_mode.as_ref(),
                            checksum_algorithm: opts.checksum_algorithm.as_ref(),
                            sse: opts.sse,
                        },
                    )
                    .await?;
                    part_cksum_val = cksum;
                    resp
                };
                // prepared.channel is dropped here and deregisters memory.
                resp
            } else {
                let (resp, cksum) = upload_part_http(
                    client,
                    chunk_buf,
                    UploadPartHttpOptions {
                        bucket,
                        key,
                        upload_id,
                        part_number,
                        checksum_mode: opts.checksum_mode.as_ref(),
                        checksum_algorithm: opts.checksum_algorithm.as_ref(),
                        sse: opts.sse,
                    },
                )
                .await?;
                part_cksum_val = cksum;
                resp
            }
        };
        #[cfg(not(feature = "rdma"))]
        let upload_part_response = {
            let (resp, cksum) = upload_part_http(
                client,
                chunk_buf,
                UploadPartHttpOptions {
                    bucket,
                    key,
                    upload_id,
                    part_number,
                    checksum_mode: opts.checksum_mode.as_ref(),
                    checksum_algorithm: opts.checksum_algorithm.as_ref(),
                    sse: opts.sse,
                },
            )
            .await?;
            part_cksum_val = cksum;
            resp
        };

        let etag = upload_part_response
            .e_tag()
            .ok_or("Failed to get ETag for part")?
            .to_string();

        parts.push(completed_part(
            part_number,
            etag,
            opts.checksum_mode.is_some(),
            &effective_algo,
            &upload_part_response,
            part_cksum_val,
        ));

        uploaded_bytes += bytes_read as u64;
        println!(
            "Uploaded part {}: {} / {} bytes ({:.1}%)",
            part_number,
            uploaded_bytes,
            file_size,
            (uploaded_bytes as f64 / file_size as f64) * 100.0
        );

        part_number += 1;

        if bytes_read < opts.multipart.chunksize as usize {
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
    opts: DownloadOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create parent directories if needed
    if let Some(parent) = Path::new(local_path).parent() {
        fs::create_dir_all(parent).await?;
    }

    // RDMA path: pre-allocate a receive buffer and inject token header.
    #[cfg(feature = "rdma")]
    if let Some(ref provider) = opts.rdma {
        let head = client.head_object().bucket(bucket).key(key).send().await?;
        let size = head.content_length().unwrap_or(0).max(0) as usize;
        let s3_key = format!("{bucket}/{key}");
        let maybe_rdma = bind_rdma_channel(provider, size, s3_key.as_bytes())
            .and_then(|channel| prepare_rdma_get_multi(&channel, size));
        let mut request = client.get_object().bucket(bucket).key(key);
        if let Some(mode) = opts.checksum_mode.clone() {
            request = request.checksum_mode(mode);
        }
        request =
            apply_sse_customer_to_get_object(request, opts.sse.destination_customer_headers()?);
        let mut file = fs::File::create(local_path).await?;
        send_get_with_optional_rdma_to_writer(request, maybe_rdma, size, &mut file).await?;
        // rdma_channel is dropped here and deregisters memory.
        println!("Downloaded: s3://{bucket}/{key} -> {local_path}");
        return Ok(());
    }

    let mut request = client.get_object().bucket(bucket).key(key);
    if let Some(mode) = opts.checksum_mode {
        request = request.checksum_mode(mode);
    }
    request = apply_sse_customer_to_get_object(request, opts.sse.destination_customer_headers()?);
    let response = request.send().await?;
    let mut file = fs::File::create(local_path).await?;
    stream_body_to_writer(response.body, &mut file).await?;
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
    sse: &SseConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let copy_source = format!("{}/{}", src_bucket, src_key);
    let mut request = client
        .copy_object()
        .copy_source(&copy_source)
        .bucket(dst_bucket)
        .key(dst_key);
    request = apply_server_side_encryption_to_copy_object(
        request,
        sse.sse.as_deref(),
        sse.sse_kms_key_id.as_deref(),
    );
    request =
        apply_destination_sse_customer_to_copy_object(request, sse.destination_customer_headers()?);
    request =
        apply_source_sse_customer_to_copy_object(request, sse.copy_source_customer_headers()?);
    request.send().await?;
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
    opts: &CopyOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    match (&source, &dest) {
        (PathType::Local(src), PathType::S3 { bucket, key }) => {
            upload_directory(client, src, bucket, key, filter, opts).await
        }
        (PathType::S3 { bucket, key }, PathType::Local(dst)) => {
            download_directory(client, bucket, key, dst, filter, opts).await
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
            copy_s3_directory(
                client, src_bucket, src_key, dst_bucket, dst_key, filter, &opts.sse,
            )
            .await
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
    opts: &CopyOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in walk_local_files(local_dir)? {
        if !filter.matches(&entry.relative) {
            continue;
        }

        let s3_key = join_s3_key(s3_prefix, &entry.relative_unix);

        upload_file(
            client,
            entry
                .path
                .to_str()
                .ok_or_else(|| format!("path contains invalid UTF-8: {}", entry.path.display()))?,
            bucket,
            &s3_key,
            UploadOptions {
                checksum_mode: None,
                checksum_algorithm: None,
                sse: &opts.sse,
                multipart: opts.multipart,
                #[cfg(feature = "rdma")]
                rdma: opts.rdma.as_ref().map(Arc::clone),
            },
        )
        .await?;
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
    opts: &CopyOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    for obj in list_s3_objects(client, bucket, prefix).await? {
        if !filter.matches(&obj.key) {
            continue;
        }

        let local_path = Path::new(local_dir).join(obj.relative_key(prefix));
        download_file(
            client,
            bucket,
            &obj.key,
            local_path
                .to_str()
                .ok_or_else(|| format!("path contains invalid UTF-8: {}", local_path.display()))?,
            DownloadOptions {
                checksum_mode: None,
                sse: &opts.sse,
                #[cfg(feature = "rdma")]
                rdma: opts.rdma.as_ref().map(Arc::clone),
            },
        )
        .await?;
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
    sse: &SseConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    for obj in list_s3_objects(client, src_bucket, src_prefix).await? {
        if !filter.matches(&obj.key) {
            continue;
        }

        let dst_key = join_s3_key(dst_prefix, obj.relative_key(src_prefix));
        copy_s3_to_s3(client, src_bucket, &obj.key, dst_bucket, &dst_key, sse).await?;
    }

    Ok(())
}

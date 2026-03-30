use crate::filters::FileFilter;
use crate::path_utils::{join_s3_key, parse_path, PathType};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{
    ChecksumAlgorithm, ChecksumMode, CompletedMultipartUpload, CompletedPart, ServerSideEncryption,
};
use aws_sdk_s3::Client;
use std::path::Path;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use walkdir::WalkDir;

use base64::{engine::general_purpose::STANDARD, Engine as _};

#[cfg(feature = "rdma")]
use crate::rdma::{RdmaInterceptor, RdmaProvider};

#[cfg(feature = "rdma")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Server-side encryption options, matching AWS S3 CLI conventions.
#[derive(Clone, Default, Debug)]
pub struct SseConfig {
    /// `--sse`: destination SSE algorithm ("AES256", "aws:kms", "aws:kms:dsse")
    pub sse: Option<String>,
    /// `--sse-kms-key-id`: AWS KMS key ARN or alias
    pub sse_kms_key_id: Option<String>,
    /// `--sse-c`: SSE-C algorithm for the destination object (typically "AES256")
    pub sse_c: Option<String>,
    /// `--sse-c-key`: base64-encoded 256-bit customer-provided key for the destination
    pub sse_c_key: Option<String>,
    /// `--sse-c-copy-source`: SSE-C algorithm for the copy source (S3-to-S3 only)
    pub sse_c_copy_source: Option<String>,
    /// `--sse-c-copy-source-key`: base64-encoded customer-provided key for the copy source
    pub sse_c_copy_source_key: Option<String>,
}

/// Compute `base64(MD5(raw_key_bytes))` required by S3 for SSE-C requests.
pub(crate) fn sse_c_key_md5(key_b64: &str) -> Result<String, Box<dyn std::error::Error>> {
    use md5::{Digest, Md5};
    let raw = STANDARD.decode(key_b64)?;
    let digest = Md5::digest(&raw);
    Ok(STANDARD.encode(digest))
}

/// Copy files between local and S3
#[allow(clippy::too_many_arguments)]
pub async fn copy(
    client: &Client,
    source: &str,
    dest: &str,
    recursive: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    checksum: Option<String>,
    sse: SseConfig,
    multipart_threshold: u64,
    multipart_chunksize: u64,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let source_type = parse_path(source)?;
    let dest_type = parse_path(dest)?;

    // Parse checksum options (only for single object operations)
    let checksum_opts = if !recursive {
        parse_checksum(checksum)?
    } else {
        if checksum.is_some() {
            eprintln!("Warning: Checksum option is ignored for recursive operations");
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
            &sse,
            multipart_threshold,
            multipart_chunksize,
            #[cfg(feature = "rdma")]
            rdma,
        )
        .await
    } else {
        copy_single(
            client,
            source_type,
            dest_type,
            checksum_opts.0,
            checksum_opts.1,
            &sse,
            multipart_threshold,
            multipart_chunksize,
            #[cfg(feature = "rdma")]
            rdma,
        )
        .await
    }
}

/// Parse checksum option into (mode, algorithm) pair
pub(crate) fn parse_checksum(
    checksum: Option<String>,
) -> Result<(Option<ChecksumMode>, Option<ChecksumAlgorithm>), String> {
    let Some(val) = checksum else {
        return Ok((None, None));
    };
    match val.to_uppercase().as_str() {
        "ENABLED" => Ok((Some(ChecksumMode::Enabled), None)),
        "CRC32" => Ok((Some(ChecksumMode::Enabled), Some(ChecksumAlgorithm::Crc32))),
        "CRC32C" => Ok((Some(ChecksumMode::Enabled), Some(ChecksumAlgorithm::Crc32C))),
        "SHA1" => Ok((Some(ChecksumMode::Enabled), Some(ChecksumAlgorithm::Sha1))),
        "SHA256" => Ok((Some(ChecksumMode::Enabled), Some(ChecksumAlgorithm::Sha256))),
        _ => Err(format!(
            "Invalid checksum value: {}. Use ENABLED, CRC32, CRC32C, SHA1, or SHA256",
            val
        )),
    }
}

/// Copy a single file
#[allow(clippy::too_many_arguments)]
async fn copy_single(
    client: &Client,
    source: PathType,
    dest: PathType,
    checksum_mode: Option<ChecksumMode>,
    checksum_algorithm: Option<ChecksumAlgorithm>,
    sse: &SseConfig,
    multipart_threshold: u64,
    multipart_chunksize: u64,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
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
                sse,
                multipart_threshold,
                multipart_chunksize,
                #[cfg(feature = "rdma")]
                rdma,
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
                checksum_mode,
                sse,
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
            // S3 to S3
            copy_s3_to_s3(client, src_bucket, src_key, dst_bucket, dst_key, sse).await
        }
        (PathType::Local(src), PathType::Local(dst)) => {
            // Local to local
            fs::copy(src, dst).await?;
            println!("Copied: {} -> {}", src, dst);
            Ok(())
        }
    }
}

/// Compute a base64-encoded checksum of `data` using the requested algorithm.
///
/// Returns `(algorithm_name, base64_value)` when checksum mode is enabled,
/// or `(None, None)` when checksums are not requested.
fn compute_put_checksum(
    data: &[u8],
    checksum_mode: Option<&ChecksumMode>,
    checksum_algorithm: Option<&ChecksumAlgorithm>,
) -> (Option<String>, Option<String>) {
    use sha1::Digest;

    if checksum_mode.is_none() {
        return (None, None);
    }
    let algo = checksum_algorithm.unwrap_or(&ChecksumAlgorithm::Crc32);
    let (name, bytes): (&str, Vec<u8>) = match algo {
        ChecksumAlgorithm::Crc32 => {
            let mut h = crc32fast::Hasher::new();
            h.update(data);
            ("CRC32", h.finalize().to_be_bytes().to_vec())
        }
        ChecksumAlgorithm::Crc32C => {
            let checksum = crc32c::crc32c(data);
            ("CRC32C", checksum.to_be_bytes().to_vec())
        }
        ChecksumAlgorithm::Sha1 => {
            let mut h = sha1::Sha1::new();
            h.update(data);
            ("SHA1", h.finalize().to_vec())
        }
        ChecksumAlgorithm::Sha256 => {
            let mut h = sha2::Sha256::new();
            h.update(data);
            ("SHA256", h.finalize().to_vec())
        }
        _ => return (None, None),
    };
    let encoded = STANDARD.encode(&bytes);
    (Some(name.to_string()), Some(encoded))
}

/// Upload a file to S3
#[allow(clippy::too_many_arguments)]
pub async fn upload_file(
    client: &Client,
    local_path: &str,
    bucket: &str,
    key: &str,
    checksum_mode: Option<ChecksumMode>,
    checksum_algorithm: Option<ChecksumAlgorithm>,
    sse: &SseConfig,
    multipart_threshold: u64,
    multipart_chunksize: u64,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
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
            checksum_mode,
            checksum_algorithm,
            sse,
            #[cfg(feature = "rdma")]
            rdma,
        )
        .await
    } else {
        // Single PUT — use RDMA when a provider is supplied.
        #[cfg(feature = "rdma")]
        if let Some(ref provider) = rdma {
            let mut buffer = tokio::fs::read(local_path).await?;
            let size = buffer.len();
            let suitable = provider.is_memory_suitable(buffer.as_ptr(), size);
            if suitable {
                provider.register_memory(buffer.as_mut_ptr(), size)?;
            }
            let maybe_token = if suitable {
                let s3_key = format!("{bucket}/{key}");
                match provider.prepare_put_token(s3_key.as_bytes(), buffer.as_ptr(), size, 0) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        if e.is_fallback_eligible() {
                            eprintln!(
                                "[rdma] prepare_put_token failed ({e}); falling back to plain HTTP"
                            );
                        } else {
                            eprintln!("[rdma] prepare_put_token error ({e})");
                        }
                        None
                    }
                }
            } else {
                None
            };
            // For RDMA transfers the data travels via RDMA, not the HTTP body.
            // Use an empty body so content-length is 0 and x-amz-content-sha256
            // reflects SHA256 of the empty string, as the server expects.
            let rdma_body = ByteStream::from_static(b"");
            if let Some(token) = maybe_token {
                let rdma_confirmed = Arc::new(AtomicBool::new(false));
                let (cksum_alg, cksum_val) = compute_put_checksum(
                    &buffer,
                    checksum_mode.as_ref(),
                    checksum_algorithm.as_ref(),
                );
                let interceptor = RdmaInterceptor::new_put(
                    Arc::clone(provider),
                    token,
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
            } else {
                client
                    .put_object()
                    .bucket(bucket)
                    .key(key)
                    .body(rdma_body)
                    .send()
                    .await?;
            }
            if suitable {
                if let Err(e) = provider.deregister_memory(buffer.as_mut_ptr()) {
                    eprintln!("[rdma] deregister_memory failed: {e}");
                }
            }
            println!("Uploaded: {local_path} -> s3://{bucket}/{key}");
            return Ok(());
        }

        let body;
        let mut request;
        if checksum_mode.is_some() {
            // Buffer the file so we can pre-compute the checksum as a plain
            // request header.  Using checksum_algorithm() on a streaming body
            // causes the SDK to use aws-chunked/trailing-checksum encoding,
            // which many S3-compatible servers do not support.
            let bytes = tokio::fs::read(local_path).await?;
            let (algo_name, cksum_val) =
                compute_put_checksum(&bytes, checksum_mode.as_ref(), checksum_algorithm.as_ref());
            body = ByteStream::from(bytes);
            request = client.put_object().bucket(bucket).key(key).body(body);
            match (algo_name.as_deref(), cksum_val) {
                (Some("CRC32"), Some(v)) => request = request.checksum_crc32(v),
                (Some("CRC32C"), Some(v)) => request = request.checksum_crc32_c(v),
                (Some("SHA1"), Some(v)) => request = request.checksum_sha1(v),
                (Some("SHA256"), Some(v)) => request = request.checksum_sha256(v),
                _ => {}
            }
        } else {
            body = ByteStream::from_path(Path::new(local_path)).await?;
            request = client.put_object().bucket(bucket).key(key).body(body);
        }
        if let Some(ref alg) = sse.sse {
            request = request.server_side_encryption(ServerSideEncryption::from(alg.as_str()));
        }
        if let Some(ref kid) = sse.sse_kms_key_id {
            request = request.ssekms_key_id(kid.clone());
        }
        if sse.sse_c.is_some() || sse.sse_c_key.is_some() {
            let algo = sse.sse_c.clone().unwrap_or_else(|| "AES256".to_string());
            request = request.sse_customer_algorithm(algo);
            if let Some(ref key) = sse.sse_c_key {
                let md5 = sse_c_key_md5(key)?;
                request = request
                    .sse_customer_key(key.clone())
                    .sse_customer_key_md5(md5);
            }
        }
        request.send().await?;
        println!("Uploaded: {} -> s3://{}/{}", local_path, bucket, key);
        Ok(())
    }
}

/// Upload a file to S3 using multipart upload
#[allow(clippy::too_many_arguments)]
async fn upload_file_multipart(
    client: &Client,
    local_path: &str,
    bucket: &str,
    key: &str,
    file_size: u64,
    chunk_size: u64,
    checksum_mode: Option<ChecksumMode>,
    checksum_algorithm: Option<ChecksumAlgorithm>,
    sse: &SseConfig,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "Using multipart upload for {} ({} bytes, {} bytes per part)",
        local_path, file_size, chunk_size
    );

    let effective_algo = checksum_algorithm
        .clone()
        .unwrap_or(ChecksumAlgorithm::Crc32);

    // Step 1: Create multipart upload
    let mut create_req = client.create_multipart_upload().bucket(bucket).key(key);
    if checksum_mode.is_some() {
        create_req = create_req.checksum_algorithm(effective_algo.clone());
    }
    if let Some(ref alg) = sse.sse {
        create_req = create_req.server_side_encryption(ServerSideEncryption::from(alg.as_str()));
    }
    if let Some(ref kid) = sse.sse_kms_key_id {
        create_req = create_req.ssekms_key_id(kid.clone());
    }
    if sse.sse_c.is_some() || sse.sse_c_key.is_some() {
        let algo = sse.sse_c.clone().unwrap_or_else(|| "AES256".to_string());
        create_req = create_req.sse_customer_algorithm(algo);
        if let Some(ref key) = sse.sse_c_key {
            let md5 = sse_c_key_md5(key)?;
            create_req = create_req
                .sse_customer_key(key.clone())
                .sse_customer_key_md5(md5);
        }
    }
    let multipart_upload = create_req.send().await?;

    let upload_id = multipart_upload
        .upload_id()
        .ok_or("Failed to get upload ID")?;

    // Step 2: Upload parts
    let mut parts = Vec::new();
    let mut file = fs::File::open(local_path).await?;
    let mut part_number = 1;
    let mut uploaded_bytes = 0u64;

    // RDMA: allocate the part buffer once before the loop so the same memory
    // region (at a stable virtual address) is reused for every part.
    // Allocating a fresh Vec each iteration risks later parts landing in pages
    // outside the CUDA-accessible region on cuobj hardware, causing
    // prepare_put_token to fail silently and losing the RDMA token.
    //
    // register_memory / deregister_memory are still called per-part because
    // providers such as MockRdmaProvider create one SHM file per registration:
    // after process_reply_token confirms a part the SHM entry is removed, so
    // the buffer must be re-registered before the next prepare_put_token.
    // For cuobj both functions are no-ops (real registration happens inside
    // prepare_put_token), so the extra calls cost nothing.
    #[cfg(feature = "rdma")]
    let (mut rdma_buffer, rdma_memory_ok) = if let Some(ref provider) = rdma {
        let buf = vec![0u8; chunk_size as usize];
        let ok = provider.is_memory_suitable(buf.as_ptr(), chunk_size as usize);
        (buf, ok)
    } else {
        (Vec::new(), false)
    };

    loop {
        let bytes_read;

        #[cfg(feature = "rdma")]
        let part_data: Option<Vec<u8>>; // Some(_) only for the non-RDMA HTTP path

        // Read the next chunk into the pre-allocated RDMA buffer (RDMA path)
        // or a fresh allocation (plain HTTP path).
        #[cfg(feature = "rdma")]
        {
            if rdma.is_some() {
                let mut n_read = 0usize;
                while n_read < chunk_size as usize {
                    let n = file.read(&mut rdma_buffer[n_read..]).await?;
                    if n == 0 {
                        break;
                    }
                    n_read += n;
                }
                if n_read == 0 {
                    break;
                }
                bytes_read = n_read;
                part_data = None;
            } else {
                let mut buffer = vec![0u8; chunk_size as usize];
                let mut n_read = 0usize;
                while n_read < chunk_size as usize {
                    let n = file.read(&mut buffer[n_read..]).await?;
                    if n == 0 {
                        break;
                    }
                    n_read += n;
                }
                if n_read == 0 {
                    break;
                }
                buffer.truncate(n_read);
                bytes_read = n_read;
                part_data = Some(buffer);
            }
        }
        #[cfg(not(feature = "rdma"))]
        let buffer;
        #[cfg(not(feature = "rdma"))]
        {
            let mut buf = vec![0u8; chunk_size as usize];
            let mut n_read = 0usize;
            while n_read < chunk_size as usize {
                let n = file.read(&mut buf[n_read..]).await?;
                if n == 0 {
                    break;
                }
                n_read += n;
            }
            if n_read == 0 {
                break;
            }
            buf.truncate(n_read);
            bytes_read = n_read;
            buffer = buf;
        }

        // Upload this part — with RDMA interceptor when a provider is present.
        #[cfg(feature = "rdma")]
        let upload_part_response = {
            if let Some(ref provider) = rdma {
                // Register the buffer for this part.  This is a no-op for cuobj
                // (real registration is inside prepare_put_token) but creates the
                // SHM entry the mock provider needs for each transfer.
                let registered = rdma_memory_ok
                    && provider
                        .register_memory(rdma_buffer.as_mut_ptr(), bytes_read)
                        .is_ok();

                let maybe_token = if registered {
                    let s3_key = format!("{bucket}/{key}");
                    provider
                        .prepare_put_token(s3_key.as_bytes(), rdma_buffer.as_ptr(), bytes_read, 0)
                        .ok()
                } else {
                    None
                };

                let resp = if let Some(token) = maybe_token {
                    let rdma_confirmed = Arc::new(AtomicBool::new(false));
                    let (cksum_alg, cksum_val) = compute_put_checksum(
                        &rdma_buffer[..bytes_read],
                        checksum_mode.as_ref(),
                        checksum_algorithm.as_ref(),
                    );
                    let interceptor = RdmaInterceptor::new_put(
                        Arc::clone(provider),
                        token,
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
                    // RDMA token unavailable — fall back to plain HTTP body.
                    let body = part_data.unwrap_or_else(|| rdma_buffer[..bytes_read].to_vec());
                    let mut req = client
                        .upload_part()
                        .bucket(bucket)
                        .key(key)
                        .upload_id(upload_id)
                        .part_number(part_number)
                        .body(ByteStream::from(body));
                    if checksum_mode.is_some() {
                        req = req.checksum_algorithm(effective_algo.clone());
                    }
                    if sse.sse_c.is_some() || sse.sse_c_key.is_some() {
                        let algo = sse.sse_c.clone().unwrap_or_else(|| "AES256".to_string());
                        req = req.sse_customer_algorithm(algo);
                        if let Some(ref key) = sse.sse_c_key {
                            let md5 = sse_c_key_md5(key)?;
                            req = req.sse_customer_key(key.clone()).sse_customer_key_md5(md5);
                        }
                    }
                    req.send().await?
                };

                // Deregister the buffer.  If process_reply_token already cleaned
                // up the entry (RDMA confirmed), this is a no-op.  For cuobj this
                // call is always a no-op.
                if registered {
                    let _ = provider.deregister_memory(rdma_buffer.as_mut_ptr());
                }
                resp
            } else {
                let mut req = client
                    .upload_part()
                    .bucket(bucket)
                    .key(key)
                    .upload_id(upload_id)
                    .part_number(part_number)
                    .body(ByteStream::from(part_data.ok_or(
                        "Internal error: part buffer missing on non-RDMA path",
                    )?));
                if checksum_mode.is_some() {
                    req = req.checksum_algorithm(effective_algo.clone());
                }
                if sse.sse_c.is_some() || sse.sse_c_key.is_some() {
                    let algo = sse.sse_c.clone().unwrap_or_else(|| "AES256".to_string());
                    req = req.sse_customer_algorithm(algo);
                    if let Some(ref key) = sse.sse_c_key {
                        let md5 = sse_c_key_md5(key)?;
                        req = req.sse_customer_key(key.clone()).sse_customer_key_md5(md5);
                    }
                }
                req.send().await?
            }
        };
        #[cfg(not(feature = "rdma"))]
        let upload_part_response = {
            let mut req = client
                .upload_part()
                .bucket(bucket)
                .key(key)
                .upload_id(upload_id)
                .part_number(part_number)
                .body(ByteStream::from(buffer));
            if checksum_mode.is_some() {
                req = req.checksum_algorithm(effective_algo.clone());
            }
            if sse.sse_c.is_some() || sse.sse_c_key.is_some() {
                let algo = sse.sse_c.clone().unwrap_or_else(|| "AES256".to_string());
                req = req.sse_customer_algorithm(algo);
                if let Some(ref key) = sse.sse_c_key {
                    let md5 = sse_c_key_md5(key)?;
                    req = req.sse_customer_key(key.clone()).sse_customer_key_md5(md5);
                }
            }
            req.send().await?
        };

        let etag = upload_part_response
            .e_tag()
            .ok_or("Failed to get ETag for part")?
            .to_string();

        let mut part_builder = CompletedPart::builder()
            .part_number(part_number)
            .e_tag(etag);
        if checksum_mode.is_some() {
            match effective_algo {
                ChecksumAlgorithm::Crc32 => {
                    if let Some(v) = upload_part_response.checksum_crc32() {
                        part_builder = part_builder.checksum_crc32(v);
                    }
                }
                ChecksumAlgorithm::Crc32C => {
                    if let Some(v) = upload_part_response.checksum_crc32_c() {
                        part_builder = part_builder.checksum_crc32_c(v);
                    }
                }
                ChecksumAlgorithm::Sha1 => {
                    if let Some(v) = upload_part_response.checksum_sha1() {
                        part_builder = part_builder.checksum_sha1(v);
                    }
                }
                ChecksumAlgorithm::Sha256 => {
                    if let Some(v) = upload_part_response.checksum_sha256() {
                        part_builder = part_builder.checksum_sha256(v);
                    }
                }
                _ => {}
            }
        }
        parts.push(part_builder.build());

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
#[allow(clippy::too_many_arguments)]
pub async fn download_file(
    client: &Client,
    bucket: &str,
    key: &str,
    local_path: &str,
    checksum_mode: Option<ChecksumMode>,
    sse: &SseConfig,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create parent directories if needed
    if let Some(parent) = Path::new(local_path).parent() {
        fs::create_dir_all(parent).await?;
    }

    // RDMA path: pre-allocate a receive buffer and inject token header.
    #[cfg(feature = "rdma")]
    if let Some(ref provider) = rdma {
        let head = client.head_object().bucket(bucket).key(key).send().await?;
        let size = head.content_length().unwrap_or(0).max(0) as usize;
        let mut buffer: Vec<u8> = vec![0u8; size];
        let suitable = size > 0 && provider.is_memory_suitable(buffer.as_ptr(), size);
        if suitable {
            provider.register_memory(buffer.as_mut_ptr(), size)?;
        }
        let maybe_token = if suitable {
            let s3_key = format!("{bucket}/{key}");
            provider
                .prepare_get_token(s3_key.as_bytes(), buffer.as_mut_ptr(), size, 0)
                .ok()
        } else {
            None
        };
        let rdma_attempted = maybe_token.is_some();
        let mut request = client.get_object().bucket(bucket).key(key);
        if let Some(mode) = checksum_mode {
            request = request.checksum_mode(mode);
        }
        if sse.sse_c.is_some() || sse.sse_c_key.is_some() {
            let algo = sse.sse_c.clone().unwrap_or_else(|| "AES256".to_string());
            request = request.sse_customer_algorithm(algo);
            if let Some(ref key_val) = sse.sse_c_key {
                let md5 = sse_c_key_md5(key_val)?;
                request = request
                    .sse_customer_key(key_val.clone())
                    .sse_customer_key_md5(md5);
            }
        }
        let rdma_confirmed = Arc::new(AtomicBool::new(false));
        let response = if let Some(token) = maybe_token {
            let interceptor = RdmaInterceptor::new_get(
                Arc::clone(provider),
                token,
                size,
                Arc::clone(&rdma_confirmed),
                false,
            );
            request.customize().interceptor(interceptor).send().await?
        } else {
            request.send().await?
        };
        let mut file = fs::File::create(local_path).await?;
        if rdma_attempted && rdma_confirmed.load(Ordering::Acquire) {
            file.write_all(&buffer[..size]).await?;
        } else {
            let mut body = response.body;
            while let Some(chunk) = body.try_next().await? {
                file.write_all(&chunk).await?;
            }
        }
        if suitable {
            let _ = provider.deregister_memory(buffer.as_mut_ptr());
        }
        println!("Downloaded: s3://{bucket}/{key} -> {local_path}");
        return Ok(());
    }

    let mut request = client.get_object().bucket(bucket).key(key);
    if let Some(mode) = checksum_mode {
        request = request.checksum_mode(mode);
    }
    if sse.sse_c.is_some() || sse.sse_c_key.is_some() {
        let algo = sse.sse_c.clone().unwrap_or_else(|| "AES256".to_string());
        request = request.sse_customer_algorithm(algo);
        if let Some(ref key_val) = sse.sse_c_key {
            let md5 = sse_c_key_md5(key_val)?;
            request = request
                .sse_customer_key(key_val.clone())
                .sse_customer_key_md5(md5);
        }
    }
    let response = request.send().await?;
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
    sse: &SseConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let copy_source = format!("{}/{}", src_bucket, src_key);
    let mut request = client
        .copy_object()
        .copy_source(&copy_source)
        .bucket(dst_bucket)
        .key(dst_key);
    if let Some(ref alg) = sse.sse {
        request = request.server_side_encryption(ServerSideEncryption::from(alg.as_str()));
    }
    if let Some(ref kid) = sse.sse_kms_key_id {
        request = request.ssekms_key_id(kid.clone());
    }
    if sse.sse_c.is_some() || sse.sse_c_key.is_some() {
        let algo = sse.sse_c.clone().unwrap_or_else(|| "AES256".to_string());
        request = request.sse_customer_algorithm(algo);
        if let Some(ref key) = sse.sse_c_key {
            let md5 = sse_c_key_md5(key)?;
            request = request
                .sse_customer_key(key.clone())
                .sse_customer_key_md5(md5);
        }
    }
    if sse.sse_c_copy_source.is_some() || sse.sse_c_copy_source_key.is_some() {
        let algo = sse
            .sse_c_copy_source
            .clone()
            .unwrap_or_else(|| "AES256".to_string());
        request = request.copy_source_sse_customer_algorithm(algo);
        if let Some(ref key) = sse.sse_c_copy_source_key {
            let md5 = sse_c_key_md5(key)?;
            request = request
                .copy_source_sse_customer_key(key.clone())
                .copy_source_sse_customer_key_md5(md5);
        }
    }
    request.send().await?;
    println!(
        "Copied: s3://{}/{} -> s3://{}/{}",
        src_bucket, src_key, dst_bucket, dst_key
    );
    Ok(())
}

/// Copy files recursively
#[allow(clippy::too_many_arguments)]
async fn copy_recursive(
    client: &Client,
    source: PathType,
    dest: PathType,
    filter: &FileFilter,
    sse: &SseConfig,
    multipart_threshold: u64,
    multipart_chunksize: u64,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    match (&source, &dest) {
        (PathType::Local(src), PathType::S3 { bucket, key }) => {
            upload_directory(
                client,
                src,
                bucket,
                key,
                filter,
                sse,
                multipart_threshold,
                multipart_chunksize,
                #[cfg(feature = "rdma")]
                rdma,
            )
            .await
        }
        (PathType::S3 { bucket, key }, PathType::Local(dst)) => {
            download_directory(
                client,
                bucket,
                key,
                dst,
                filter,
                sse,
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
            copy_s3_directory(
                client, src_bucket, src_key, dst_bucket, dst_key, filter, sse,
            )
            .await
        }
        (PathType::Local(_), PathType::Local(_)) => Err(
            "Local to local recursive copy not implemented. Use standard 'cp -r' command.".into(),
        ),
    }
}

/// Upload a directory to S3
#[allow(clippy::too_many_arguments)]
async fn upload_directory(
    client: &Client,
    local_dir: &str,
    bucket: &str,
    s3_prefix: &str,
    filter: &FileFilter,
    sse: &SseConfig,
    multipart_threshold: u64,
    multipart_chunksize: u64,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let base_path = Path::new(local_dir);

    for entry in WalkDir::new(local_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();

        if path.is_file() {
            let relative_path = path
                .strip_prefix(base_path)
                .map_err(|e| format!("Path error: {}", e))?;
            let relative_str = relative_path.to_string_lossy().to_string();

            if !filter.matches(&relative_str) {
                continue;
            }

            let s3_key = join_s3_key(s3_prefix, &relative_str.replace("\\", "/"));

            upload_file(
                client,
                path.to_str()
                    .ok_or_else(|| format!("path contains invalid UTF-8: {}", path.display()))?,
                bucket,
                &s3_key,
                None,
                None,
                sse,
                multipart_threshold,
                multipart_chunksize,
                #[cfg(feature = "rdma")]
                rdma.as_ref().map(Arc::clone),
            )
            .await?;
        }
    }

    Ok(())
}

/// Download S3 prefix to local directory
#[allow(clippy::too_many_arguments)]
async fn download_directory(
    client: &Client,
    bucket: &str,
    prefix: &str,
    local_dir: &str,
    filter: &FileFilter,
    sse: &SseConfig,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
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
                if !filter.matches(key) {
                    continue;
                }

                let relative_key = if !prefix.is_empty() && key.starts_with(prefix) {
                    key[prefix.len()..].trim_start_matches('/')
                } else {
                    key
                };

                let local_path = Path::new(local_dir).join(relative_key);
                download_file(
                    client,
                    bucket,
                    key,
                    local_path.to_str().ok_or_else(|| {
                        format!("path contains invalid UTF-8: {}", local_path.display())
                    })?,
                    None,
                    sse,
                    #[cfg(feature = "rdma")]
                    rdma.as_ref().map(Arc::clone),
                )
                .await?;
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
    sse: &SseConfig,
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
                copy_s3_to_s3(client, src_bucket, key, dst_bucket, &dst_key, sse).await?;
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

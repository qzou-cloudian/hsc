use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{self, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

#[cfg(feature = "rdma")]
use crate::rdma::{RdmaInterceptor, RdmaEndpoint};
#[cfg(feature = "rdma")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Concatenate and print file or object content to STDOUT
#[allow(clippy::too_many_arguments)]
pub async fn cat(
    client: &Client,
    path: &str,
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
    part_number: Option<i32>,
    version_id: Option<String>,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaEndpoint>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Validate options
    if range.is_some() && (offset.is_some() || size.is_some()) {
        return Err("Cannot specify both --range and --offset/--size options".into());
    }
    if part_number.is_some() && (range.is_some() || offset.is_some() || size.is_some()) {
        return Err("Cannot specify --part-number together with --range or --offset/--size".into());
    }
    if let Some(p) = part_number {
        if !(1..=10000).contains(&p) {
            return Err(format!("--part-number must be between 1 and 10000, got {}", p).into());
        }
    }

    let path_type = parse_path(path)?;

    match path_type {
        PathType::S3 { bucket, key } => {
            if key.is_empty() {
                return Err("Cannot cat an S3 bucket, please specify an object key".into());
            }
            cat_s3_object(
                client,
                &bucket,
                &key,
                S3CatOptions {
                    range,
                    offset,
                    size,
                    part_number,
                    version_id,
                    #[cfg(feature = "rdma")]
                    rdma,
                },
            )
            .await
        }
        PathType::Local(local_path) => {
            if part_number.is_some() {
                return Err("--part-number is only supported for S3 objects".into());
            }
            if version_id.is_some() {
                return Err("--version-id is only supported for S3 objects".into());
            }
            cat_local_file(&local_path, range, offset, size).await
        }
    }
}

struct S3CatOptions {
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
    part_number: Option<i32>,
    version_id: Option<String>,
    #[cfg(feature = "rdma")]
    rdma: Option<Arc<dyn RdmaEndpoint>>,
}

/// Read and output S3 object content
async fn cat_s3_object(
    client: &Client,
    bucket: &str,
    key: &str,
    opts: S3CatOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let S3CatOptions {
        range,
        offset,
        size,
        part_number,
        version_id,
        #[cfg(feature = "rdma")]
        rdma,
    } = opts;
    // Build the Range header string (if any).
    let range_hdr = if let Some(range_str) = range {
        Some(if range_str.starts_with("bytes=") {
            range_str
        } else {
            format!("bytes={}", range_str)
        })
    } else {
        offset.map(|start| {
            if let Some(len) = size {
                format!("bytes={}-{}", start, start + len - 1)
            } else {
                format!("bytes={}-", start)
            }
        })
    };

    // When RDMA is enabled and we know the response size, use the RDMA path.
    #[cfg(feature = "rdma")]
    if let Some(ref provider) = rdma {
        // We need the byte count to allocate a receive buffer.  A HEAD request
        // gives us Content-Length; for ranged requests we compute from the range.
        let byte_count: Option<usize> = if let Some(ref r) = range_hdr {
            // Parse "bytes=start-end" to derive length
            r.strip_prefix("bytes=").and_then(|s| {
                let mut parts = s.splitn(2, '-');
                let start = parts.next()?.parse::<u64>().ok()?;
                let end_str = parts.next()?;
                if end_str.is_empty() {
                    None // open-ended range — size unknown without HEAD
                } else {
                    let end = end_str.parse::<u64>().ok()?;
                    Some((end - start + 1) as usize)
                }
            })
        } else {
            // Full object — HEAD for size
            let mut head_req = client.head_object().bucket(bucket).key(key);
            if let Some(ref v) = version_id {
                head_req = head_req.version_id(v.clone());
            }
            head_req
                .send()
                .await
                .ok()
                .and_then(|h| h.content_length())
                .map(|n| n.max(0) as usize)
        };

        if let Some(size) = byte_count {
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
            if let Some(ref r) = range_hdr {
                request = request.range(r.clone());
            }
            if let Some(p) = part_number {
                request = request.part_number(p);
            }
            if let Some(ref v) = version_id {
                request = request.version_id(v.clone());
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
            let mut stdout = io::stdout();
            if rdma_attempted && rdma_confirmed.load(Ordering::Acquire) {
                stdout.write_all(&buffer[..size]).await?;
            } else {
                let mut body = response.body;
                while let Some(bytes) = body.try_next().await? {
                    stdout.write_all(&bytes).await?;
                }
            }
            if suitable {
                let _ = provider.deregister_memory(buffer.as_mut_ptr());
            }
            stdout.flush().await?;
            return Ok(());
        }
    }

    let mut request = client.get_object().bucket(bucket).key(key);
    if let Some(r) = range_hdr {
        request = request.range(r);
    }
    if let Some(p) = part_number {
        request = request.part_number(p);
    }
    if let Some(v) = version_id {
        request = request.version_id(v);
    }
    let response = request.send().await?;
    let mut body = response.body;
    let mut stdout = io::stdout();
    while let Some(bytes) = body.try_next().await? {
        stdout.write_all(&bytes).await?;
    }
    stdout.flush().await?;

    Ok(())
}

/// Read and output local file content
async fn cat_local_file(
    path: &str,
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path_obj = Path::new(path);

    if !path_obj.exists() {
        return Err(format!("File '{}' does not exist", path).into());
    }

    if !path_obj.is_file() {
        return Err(format!("'{}' is not a file", path).into());
    }

    let mut file = File::open(path_obj).await?;
    let mut stdout = io::stdout();

    // Parse range options
    let (start_pos, read_size) = parse_range_options(range, offset, size)?;

    if let Some(start) = start_pos {
        file.seek(io::SeekFrom::Start(start)).await?;
    }

    // Read and output file content
    if let Some(size) = read_size {
        // Read specific size
        let mut buffer = vec![0u8; 8192];
        let mut remaining = size;

        while remaining > 0 {
            let to_read = std::cmp::min(buffer.len() as u64, remaining) as usize;
            let n = file.read(&mut buffer[..to_read]).await?;

            if n == 0 {
                break; // EOF
            }

            stdout.write_all(&buffer[..n]).await?;
            remaining -= n as u64;
        }
    } else {
        // Read entire file (or from offset to end)
        let mut buffer = vec![0u8; 8192];

        loop {
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            stdout.write_all(&buffer[..n]).await?;
        }
    }

    stdout.flush().await?;

    Ok(())
}

/// Parse range options into (start_position, size_to_read)
fn parse_range_options(
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
) -> Result<(Option<u64>, Option<u64>), Box<dyn std::error::Error>> {
    if let Some(range_str) = range {
        // Parse range string like "0-100" or "bytes=0-100"
        let range_part = range_str.strip_prefix("bytes=").unwrap_or(&range_str);

        let parts: Vec<&str> = range_part.split('-').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid range format: '{}'. Expected format: 'start-end' or 'start-'",
                range_str
            )
            .into());
        }

        let start = parts[0]
            .parse::<u64>()
            .map_err(|_| format!("Invalid start position in range: '{}'", parts[0]))?;

        if parts[1].is_empty() {
            // Open-ended range like "100-"
            Ok((Some(start), None))
        } else {
            let end = parts[1]
                .parse::<u64>()
                .map_err(|_| format!("Invalid end position in range: '{}'", parts[1]))?;

            if end < start {
                return Err("End position must be greater than or equal to start position".into());
            }

            let size = end - start + 1;
            Ok((Some(start), Some(size)))
        }
    } else if let Some(start) = offset {
        Ok((Some(start), size))
    } else {
        Ok((None, None))
    }
}

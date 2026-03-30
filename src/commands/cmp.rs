use crate::commands::cp::sse_c_key_md5;
use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[cfg(feature = "rdma")]
use crate::rdma::{RdmaInterceptor, RdmaProvider};
#[cfg(feature = "rdma")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const CHUNK_SIZE: usize = 65536; // 64KB read buffer

/// Compare two files or objects byte-by-byte, with optional range/offset/size.
/// Prints nothing if identical, or the first differing byte offset if different.
pub async fn cmp(
    client: &Client,
    path1: &str,
    path2: &str,
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
    sse_c: Option<String>,
    sse_c_key: Option<String>,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if range.is_some() && (offset.is_some() || size.is_some()) {
        return Err("Cannot specify both --range and --offset/--size".into());
    }

    let (start, limit) = resolve_range(range, offset, size)?;

    let mut reader1 = open_reader(
        client,
        path1,
        start,
        limit,
        sse_c.as_deref(),
        sse_c_key.as_deref(),
        #[cfg(feature = "rdma")]
        rdma.as_ref().map(Arc::clone),
    )
    .await?;
    let mut reader2 = open_reader(
        client,
        path2,
        start,
        limit,
        sse_c.as_deref(),
        sse_c_key.as_deref(),
        #[cfg(feature = "rdma")]
        rdma,
    )
    .await?;

    let (size1, size2) = (reader1.total_size, reader2.total_size);

    let mut buf1 = vec![0u8; CHUNK_SIZE];
    let mut buf2 = vec![0u8; CHUNK_SIZE];
    let mut byte_pos: u64 = start.unwrap_or(0);
    let mut remaining = limit;

    loop {
        let to_read = remaining
            .map(|r| r.min(CHUNK_SIZE as u64) as usize)
            .unwrap_or(CHUNK_SIZE);

        if to_read == 0 {
            break;
        }

        let n1 = read_exact_or_eof(&mut reader1, &mut buf1[..to_read]).await?;
        let n2 = read_exact_or_eof(&mut reader2, &mut buf2[..to_read]).await?;

        // Compare the bytes that were actually read
        let n = n1.min(n2);
        for i in 0..n {
            if buf1[i] != buf2[i] {
                eprintln!(
                    "{} {} differ: byte {}, line {}",
                    path1,
                    path2,
                    byte_pos + i as u64 + 1, // 1-based, like cmp(1)
                    count_lines(&buf1[..i]) + 1
                );
                std::process::exit(1);
            }
        }

        byte_pos += n as u64;

        // Handle EOF differences
        if n1 != n2 {
            let shorter = if n1 < n2 { path1 } else { path2 };
            eprintln!("cmp: EOF on {}", shorter);
            std::process::exit(1);
        }

        if n1 == 0 {
            break; // both EOF
        }

        if let Some(ref mut r) = remaining {
            *r -= n as u64;
        }
    }

    // When no range is given, also compare total sizes
    if limit.is_none() && size1 != size2 {
        let shorter = if size1 < size2 { path1 } else { path2 };
        eprintln!("cmp: EOF on {}", shorter);
        std::process::exit(1);
    }

    // Files are identical (within range)
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Parse range/offset/size into (start, limit) byte counts.
fn resolve_range(
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
) -> Result<(Option<u64>, Option<u64>), Box<dyn std::error::Error>> {
    if let Some(range_str) = range {
        let part = range_str.strip_prefix("bytes=").unwrap_or(&range_str);
        let parts: Vec<&str> = part.split('-').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid range '{}', expected 'start-end'", range_str).into());
        }
        let start = parts[0]
            .parse::<u64>()
            .map_err(|_| format!("Invalid range start '{}'", parts[0]))?;
        let limit = if parts[1].is_empty() {
            None
        } else {
            let end = parts[1]
                .parse::<u64>()
                .map_err(|_| format!("Invalid range end '{}'", parts[1]))?;
            if end < start {
                return Err("Range end must be >= start".into());
            }
            Some(end - start + 1)
        };
        Ok((Some(start), limit))
    } else {
        Ok((offset, size))
    }
}

/// Count newlines in a byte slice (for line-number reporting).
fn count_lines(buf: &[u8]) -> u64 {
    buf.iter().filter(|&&b| b == b'\n').count() as u64
}

// ── abstracted reader ─────────────────────────────────────────────────────────

struct Reader {
    inner: ReaderInner,
    total_size: u64,
}

enum ReaderInner {
    Local(File),
    S3 { data: Vec<u8>, pos: usize },
}

/// Open a local file or S3 object as a Reader, seeking/slicing to the given start.
async fn open_reader(
    client: &Client,
    path: &str,
    start: Option<u64>,
    limit: Option<u64>,
    sse_c: Option<&str>,
    sse_c_key: Option<&str>,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<Reader, Box<dyn std::error::Error>> {
    match parse_path(path)? {
        PathType::Local(local_path) => {
            let meta = tokio::fs::metadata(&local_path)
                .await
                .map_err(|e| format!("Cannot access '{}': {}", local_path, e))?;
            if !meta.is_file() {
                return Err(format!("'{}' is not a file", local_path).into());
            }
            let total_size = meta.len();
            let mut file = File::open(Path::new(&local_path)).await?;
            if let Some(s) = start {
                file.seek(tokio::io::SeekFrom::Start(s)).await?;
            }
            Ok(Reader {
                inner: ReaderInner::Local(file),
                total_size,
            })
        }
        PathType::S3 { bucket, key } => {
            if key.is_empty() {
                return Err(format!("'{}' is an S3 bucket, not an object", path).into());
            }

            // HEAD to get total size
            let mut head_req = client.head_object().bucket(&bucket).key(&key);
            if let (Some(algo), Some(key_b64)) = (sse_c, sse_c_key) {
                let md5 = sse_c_key_md5(key_b64)?;
                head_req = head_req
                    .sse_customer_algorithm(algo)
                    .sse_customer_key(key_b64)
                    .sse_customer_key_md5(md5);
            }
            let head = head_req
                .send()
                .await
                .map_err(|e| format!("Cannot stat s3://{}/{}: {}", bucket, key, e))?;
            let total_size = head.content_length().unwrap_or(0) as u64;

            // Byte count for this particular request (may be a sub-range).
            // Only used in the RDMA path below; suppress the warning for non-rdma builds.
            #[cfg_attr(not(feature = "rdma"), allow(unused_variables))]
            let byte_count = limit.unwrap_or(total_size) as usize;

            // Build Range header
            let range_hdr = build_range_header(start, limit);

            // Try RDMA path when a provider is supplied.
            #[cfg(feature = "rdma")]
            if let Some(ref provider) = rdma {
                let mut buffer: Vec<u8> = vec![0u8; byte_count];
                let raw_ptr = buffer.as_mut_ptr(); // save before any potential move
                let suitable =
                    byte_count > 0 && provider.is_memory_suitable(buffer.as_ptr(), byte_count);
                if suitable {
                    let _ = provider.register_memory(buffer.as_mut_ptr(), byte_count);
                }
                let maybe_token = if suitable {
                    let s3_key = format!("{}/{}", bucket, key);
                    provider
                        .prepare_get_token(s3_key.as_bytes(), buffer.as_mut_ptr(), byte_count, 0)
                        .ok()
                } else {
                    None
                };
                let rdma_attempted = maybe_token.is_some();
                let mut req = client.get_object().bucket(&bucket).key(&key);
                if let Some(ref r) = range_hdr {
                    req = req.range(r.clone());
                }
                let rdma_confirmed = Arc::new(AtomicBool::new(false));
                let resp = if let Some(token) = maybe_token {
                    let interceptor = RdmaInterceptor::new_get(
                        Arc::clone(provider),
                        token,
                        byte_count,
                        Arc::clone(&rdma_confirmed),
                        false,
                    );
                    req.customize().interceptor(interceptor).send().await?
                } else {
                    req.send().await?
                };
                let bytes = if rdma_attempted && rdma_confirmed.load(Ordering::Acquire) {
                    buffer
                } else {
                    resp.body.collect().await?.into_bytes().to_vec()
                };
                // deregister via the raw pointer (buffer may have been moved into `bytes`)
                if suitable {
                    let _ = provider.deregister_memory(raw_ptr);
                }
                return Ok(Reader {
                    inner: ReaderInner::S3 {
                        data: bytes,
                        pos: 0,
                    },
                    total_size,
                });
            }

            let mut req = client.get_object().bucket(&bucket).key(&key);
            if let Some(r) = range_hdr {
                req = req.range(r);
            }
            if let (Some(algo), Some(key_b64)) = (sse_c, sse_c_key) {
                let md5 = sse_c_key_md5(key_b64)?;
                req = req
                    .sse_customer_algorithm(algo)
                    .sse_customer_key(key_b64)
                    .sse_customer_key_md5(md5);
            }
            let resp = req.send().await?;
            let bytes = resp.body.collect().await?.into_bytes().to_vec();
            Ok(Reader {
                inner: ReaderInner::S3 {
                    data: bytes,
                    pos: 0,
                },
                total_size,
            })
        }
    }
}

fn build_range_header(start: Option<u64>, limit: Option<u64>) -> Option<String> {
    match (start, limit) {
        (Some(s), Some(l)) => Some(format!("bytes={}-{}", s, s + l - 1)),
        (Some(s), None) => Some(format!("bytes={}-", s)),
        _ => None,
    }
}

/// Read up to `buf.len()` bytes; return how many were actually read (0 = EOF).
async fn read_exact_or_eof(
    reader: &mut Reader,
    buf: &mut [u8],
) -> Result<usize, Box<dyn std::error::Error>> {
    match &mut reader.inner {
        ReaderInner::Local(f) => {
            let mut total = 0;
            while total < buf.len() {
                let n = f.read(&mut buf[total..]).await?;
                if n == 0 {
                    break;
                }
                total += n;
            }
            Ok(total)
        }
        ReaderInner::S3 { data, pos } => {
            let available = (data.len() - *pos).min(buf.len());
            if available == 0 {
                return Ok(0);
            }
            buf[..available].copy_from_slice(&data[*pos..*pos + available]);
            *pos += available;
            Ok(available)
        }
    }
}

use crate::commands::cp::sse_c_key_md5;
use crate::commands::hash::{compute_hash_for_path, HashOutput};
use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use serde::Serialize;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[cfg(feature = "rdma")]
use crate::rdma::{RdmaInterceptor, RdmaClientProvider, RdmaClientChannel};
#[cfg(feature = "rdma")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const CHUNK_SIZE: usize = 65536; // 64 KiB read buffer

/// First-difference detail from a byte-by-byte comparison.
#[derive(Debug, Serialize)]
pub struct CompareDifference {
    pub kind: String,
    pub byte: Option<u64>,
    pub line: Option<u64>,
    pub shorter_path: Option<String>,
}

/// Full result returned by `compare_paths`.
#[derive(Debug, Serialize)]
pub struct CompareReport {
    pub identical: bool,
    pub path1: String,
    pub path2: String,
    pub range_start: Option<u64>,
    pub range_size: Option<u64>,
    pub bytes_compared: u64,
    pub size1: Option<u64>,
    pub size2: Option<u64>,
    pub difference: Option<CompareDifference>,
}

/// JSON envelope emitted by `hsc cmp --json`.
#[derive(Serialize)]
struct CmpOutput {
    identical: bool,
    compare: CompareReport,
    hash: Option<CmpHashPair>,
}

/// Per-path hash included in `CmpOutput` when the comparison succeeds (full-file only).
#[derive(Serialize)]
struct CmpHashPair {
    algorithm: String,
    path1: HashOutput,
    path2: HashOutput,
}

#[allow(clippy::too_many_arguments)]
pub async fn cmp(
    client: &Client,
    path1: &str,
    path2: &str,
    algorithm: &str,
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
    sse_c: Option<String>,
    sse_c_key: Option<String>,
    json: bool,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaClientProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let has_range = range.is_some() || offset.is_some() || size.is_some();
    let compare = compare_paths(
        client,
        path1,
        path2,
        range,
        offset,
        size,
        sse_c,
        sse_c_key,
        #[cfg(feature = "rdma")]
        rdma,
    )
    .await?;

    // Hash is only meaningful for a full-file comparison; skip when a byte range is active.
    let hash = if compare.identical && !has_range {
        Some(CmpHashPair {
            algorithm: algorithm.to_ascii_uppercase(),
            path1: compute_hash_for_path(client, path1, algorithm).await?,
            path2: compute_hash_for_path(client, path2, algorithm).await?,
        })
    } else {
        None
    };

    let output = CmpOutput {
        identical: compare.identical,
        compare,
        hash,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if output.identical {
        println!("identical: true");
        if let Some(hash) = &output.hash {
            println!("algorithm: {}", hash.algorithm);
            println!("{}: {}", hash.path1.path, hash.path1.value);
            println!("{}: {}", hash.path2.path, hash.path2.value);
        }
    } else {
        println!("identical: false");
        if let Some(difference) = &output.compare.difference {
            print_difference(difference);
        }
    }

    if output.identical {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn print_difference(difference: &CompareDifference) {
    match difference.kind.as_str() {
        "byte_mismatch" => {
            if let Some(byte) = difference.byte {
                if let Some(line) = difference.line {
                    println!("reason: content differs at byte {}, line {}", byte, line);
                } else {
                    println!("reason: content differs at byte {}", byte);
                }
            } else {
                println!("reason: content differs");
            }
        }
        "eof" => {
            if let Some(ref shorter) = difference.shorter_path {
                println!("reason: EOF on {}", shorter);
            } else {
                println!("reason: size differs");
            }
        }
        _ => println!("reason: {}", difference.kind),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn compare_paths(
    client: &Client,
    path1: &str,
    path2: &str,
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
    sse_c: Option<String>,
    sse_c_key: Option<String>,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaClientProvider>>,
) -> Result<CompareReport, Box<dyn std::error::Error>> {
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
    let mut line_pos: u64 = 1;
    let mut bytes_compared = 0u64;

    loop {
        let to_read = remaining
            .map(|r| r.min(CHUNK_SIZE as u64) as usize)
            .unwrap_or(CHUNK_SIZE);

        if to_read == 0 {
            break;
        }

        let n1 = read_exact_or_eof(&mut reader1, &mut buf1[..to_read]).await?;
        let n2 = read_exact_or_eof(&mut reader2, &mut buf2[..to_read]).await?;

        let n = n1.min(n2);
        for i in 0..n {
            if buf1[i] != buf2[i] {
                return Ok(CompareReport {
                    identical: false,
                    path1: path1.to_string(),
                    path2: path2.to_string(),
                    range_start: start,
                    range_size: limit,
                    bytes_compared: bytes_compared + i as u64,
                    size1: (limit.is_none()).then_some(size1),
                    size2: (limit.is_none()).then_some(size2),
                    difference: Some(CompareDifference {
                        kind: "byte_mismatch".to_string(),
                        byte: Some(byte_pos + i as u64 + 1),
                        line: Some(line_pos + count_lines(&buf1[..i])),
                        shorter_path: None,
                    }),
                });
            }
        }

        bytes_compared += n as u64;
        byte_pos += n as u64;
        line_pos += count_lines(&buf1[..n]);

        if n1 != n2 {
            let shorter = if n1 < n2 { path1 } else { path2 };
            return Ok(CompareReport {
                identical: false,
                path1: path1.to_string(),
                path2: path2.to_string(),
                range_start: start,
                range_size: limit,
                bytes_compared,
                size1: (limit.is_none()).then_some(size1),
                size2: (limit.is_none()).then_some(size2),
                difference: Some(CompareDifference {
                    kind: "eof".to_string(),
                    byte: None,
                    line: None,
                    shorter_path: Some(shorter.to_string()),
                }),
            });
        }

        if n1 == 0 {
            break;
        }

        if let Some(ref mut r) = remaining {
            *r -= n as u64;
        }
    }

    if limit.is_none() && size1 != size2 {
        let shorter = if size1 < size2 { path1 } else { path2 };
        return Ok(CompareReport {
            identical: false,
            path1: path1.to_string(),
            path2: path2.to_string(),
            range_start: start,
            range_size: limit,
            bytes_compared,
            size1: Some(size1),
            size2: Some(size2),
            difference: Some(CompareDifference {
                kind: "eof".to_string(),
                byte: None,
                line: None,
                shorter_path: Some(shorter.to_string()),
            }),
        });
    }

    Ok(CompareReport {
        identical: true,
        path1: path1.to_string(),
        path2: path2.to_string(),
        range_start: start,
        range_size: limit,
        bytes_compared,
        size1: (limit.is_none()).then_some(size1),
        size2: (limit.is_none()).then_some(size2),
        difference: None,
    })
}

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

fn count_lines(buf: &[u8]) -> u64 {
    buf.iter().filter(|&&b| b == b'\n').count() as u64
}

struct Reader {
    inner: ReaderInner,
    total_size: u64,
}

enum ReaderInner {
    Local(File),
    S3 { data: Vec<u8>, pos: usize },
}

async fn open_reader(
    client: &Client,
    path: &str,
    start: Option<u64>,
    limit: Option<u64>,
    sse_c: Option<&str>,
    sse_c_key: Option<&str>,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaClientProvider>>,
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

            let total_size = if limit.is_none() {
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
                head.content_length().unwrap_or(0) as u64
            } else {
                0
            };

            #[cfg_attr(not(feature = "rdma"), allow(unused_variables))]
            let byte_count = limit.unwrap_or(total_size) as usize;
            let range_hdr = build_range_header(start, limit);

            #[cfg(feature = "rdma")]
            if let Some(ref provider) = rdma {
                let mut buffer: Vec<u8> = vec![0u8; byte_count];
                let s3_key = format!("{}/{}", bucket, key);
                let maybe_channel: Option<Box<dyn RdmaClientChannel>> =
                    if byte_count > 0 && provider.is_memory_suitable(buffer.as_ptr(), byte_count) {
                        provider.bind(buffer.as_mut_ptr(), byte_count, s3_key.as_bytes()).ok()
                    } else {
                        None
                    };
                let maybe_token = if let Some(ref channel) = maybe_channel {
                    channel.prepare_get_token(0, byte_count).ok()
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
                    let channel_arc: Arc<dyn RdmaClientChannel> =
                        Arc::from(maybe_channel.unwrap());
                    let interceptor = RdmaInterceptor::new_get(
                        channel_arc,
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
                // maybe_channel dropped here (or consumed above) → deregisters memory.
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

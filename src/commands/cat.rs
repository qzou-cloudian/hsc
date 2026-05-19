use crate::commands::range::{
    bounded_len, build_range_header, parse_range_options, RangeErrorStyle,
};
use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{self, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

#[cfg(feature = "rdma")]
use crate::commands::transfer::{
    bind_rdma_channel, prepare_rdma_get_single, send_get_with_optional_rdma_to_writer,
};
#[cfg(feature = "rdma")]
use crate::rdma::RdmaClientProvider;
#[cfg(feature = "rdma")]
use std::sync::Arc;

/// Concatenate and print file or object content to STDOUT
pub struct CatOptions {
    pub range: Option<String>,
    pub offset: Option<u64>,
    pub size: Option<u64>,
    pub part_number: Option<i32>,
    pub version_id: Option<String>,
    #[cfg(feature = "rdma")]
    pub rdma: Option<Arc<dyn RdmaClientProvider>>,
}

pub async fn cat(
    client: &Client,
    path: &str,
    opts: CatOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    // Validate options
    if opts.range.is_some() && (opts.offset.is_some() || opts.size.is_some()) {
        return Err("Cannot specify both --range and --offset/--size options".into());
    }
    if opts.part_number.is_some()
        && (opts.range.is_some() || opts.offset.is_some() || opts.size.is_some())
    {
        return Err("Cannot specify --part-number together with --range or --offset/--size".into());
    }
    if let Some(p) = opts.part_number {
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
            cat_s3_object(client, &bucket, &key, opts).await
        }
        PathType::Local(local_path) => {
            if opts.part_number.is_some() {
                return Err("--part-number is only supported for S3 objects".into());
            }
            if opts.version_id.is_some() {
                return Err("--version-id is only supported for S3 objects".into());
            }
            cat_local_file(&local_path, opts.range, opts.offset, opts.size).await
        }
    }
}

/// Read and output S3 object content
async fn cat_s3_object(
    client: &Client,
    bucket: &str,
    key: &str,
    opts: CatOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let CatOptions {
        range,
        offset,
        size,
        part_number,
        version_id,
        #[cfg(feature = "rdma")]
        rdma,
    } = opts;
    let byte_range = parse_range_options(range, offset, size, RangeErrorStyle::Cat)?;
    let range_hdr = build_range_header(byte_range);
    let _range_len = bounded_len(byte_range);

    // When RDMA is enabled and we know the response size, use the RDMA path.
    #[cfg(feature = "rdma")]
    if let Some(ref provider) = rdma {
        // We need the byte count to allocate a receive buffer.  A HEAD request
        // gives us Content-Length; for ranged requests we compute from the range.
        let byte_count: Option<usize> = if range_hdr.is_some() {
            _range_len
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
            let s3_key = format!("{bucket}/{key}");
            let maybe_rdma = bind_rdma_channel(provider, size, s3_key.as_bytes())
                .and_then(|channel| prepare_rdma_get_single(&channel, size));
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
            let mut stdout = io::stdout();
            send_get_with_optional_rdma_to_writer(request, maybe_rdma, size, &mut stdout).await?;
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
    let byte_range = parse_range_options(range, offset, size, RangeErrorStyle::Cat)?;

    if let Some(start) = byte_range.start {
        file.seek(io::SeekFrom::Start(start)).await?;
    }

    // Read and output file content
    if let Some(size) = byte_range.len {
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

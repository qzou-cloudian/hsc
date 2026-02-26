use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{self, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Concatenate and print file or object content to STDOUT
pub async fn cat(
    client: &Client,
    path: &str,
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Validate options
    if range.is_some() && (offset.is_some() || size.is_some()) {
        return Err("Cannot specify both --range and --offset/--size options".into());
    }

    let path_type = parse_path(path)?;

    match path_type {
        PathType::S3 { bucket, key } => {
            if key.is_empty() {
                return Err("Cannot cat an S3 bucket, please specify an object key".into());
            }
            cat_s3_object(client, &bucket, &key, range, offset, size).await
        }
        PathType::Local(local_path) => cat_local_file(&local_path, range, offset, size).await,
    }
}

/// Read and output S3 object content
async fn cat_s3_object(
    client: &Client,
    bucket: &str,
    key: &str,
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut request = client.get_object().bucket(bucket).key(key);

    // Handle range options
    if let Some(range_str) = range {
        // Normalize range format (accept "0-100" or "bytes=0-100")
        let normalized = if range_str.starts_with("bytes=") {
            range_str
        } else {
            format!("bytes={}", range_str)
        };
        request = request.range(normalized);
    } else if let Some(start) = offset {
        // Build range from offset and size
        let range_str = if let Some(len) = size {
            format!("bytes={}-{}", start, start + len - 1)
        } else {
            format!("bytes={}-", start)
        };
        request = request.range(range_str);
    }

    let response = request.send().await?;
    let mut body = response.body;

    // Stream output to STDOUT
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
        let range_part = if range_str.starts_with("bytes=") {
            &range_str[6..]
        } else {
            &range_str[..]
        };

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

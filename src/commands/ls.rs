use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;

/// List S3 buckets or objects
pub async fn list(
    client: &Client,
    path: Option<String>,
    recursive: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match path {
        None => {
            // List all buckets
            list_buckets(client).await
        }
        Some(path_str) => {
            let path_type = parse_path(&path_str)?;
            match path_type {
                PathType::S3 { bucket, key } => {
                    list_objects(client, &bucket, &key, recursive).await
                }
                PathType::Local(_) => {
                    Err("ls command requires S3 URI (s3://bucket[/prefix])".into())
                }
            }
        }
    }
}

/// List all S3 buckets
async fn list_buckets(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    let response = client.list_buckets().send().await?;

    let buckets = response.buckets();
    if buckets.is_empty() {
        println!("No buckets found");
    } else {
        for bucket in buckets {
            if let Some(name) = bucket.name() {
                let creation_date = bucket
                    .creation_date()
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "N/A".to_string());
                println!("{:30} {}", creation_date, name);
            }
        }
        println!("\nTotal buckets: {}", buckets.len());
    }

    Ok(())
}

/// List objects in a bucket with optional prefix
async fn list_objects(
    client: &Client,
    bucket: &str,
    prefix: &str,
    recursive: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut continuation_token: Option<String> = None;
    let mut total_count = 0;
    let mut total_size = 0i64;

    loop {
        let mut request = client.list_objects_v2().bucket(bucket);

        if !prefix.is_empty() {
            request = request.prefix(prefix);
        }

        if !recursive {
            // Use delimiter to get only immediate children
            request = request.delimiter("/");
        }

        if let Some(token) = continuation_token {
            request = request.continuation_token(token);
        }

        let response = request.send().await?;

        // List common prefixes (directories) when not recursive
        if !recursive {
            for common_prefix in response.common_prefixes() {
                if let Some(prefix_str) = common_prefix.prefix() {
                    println!("{:>20} {}", "PRE", prefix_str);
                }
            }
        }

        // List objects
        for obj in response.contents() {
            if let Some(key) = obj.key() {
                let size = obj.size().unwrap_or(0);
                let last_modified = obj
                    .last_modified()
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "N/A".to_string());

                println!("{:30} {:>12} {}", last_modified, size, key);
                total_count += 1;
                total_size += size;
            }
        }

        if response.is_truncated() == Some(true) {
            continuation_token = response.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    println!(
        "\nTotal objects: {}, Total size: {} bytes",
        total_count, total_size
    );
    Ok(())
}

use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use aws_smithy_types::date_time::Format;

/// List S3 buckets or objects
pub async fn list(
    client: &Client,
    path: Option<String>,
    recursive: bool,
    versions: bool,
    human_readable: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match path {
        None => {
            // List all buckets — --versions / --human-readable don't apply here
            list_buckets(client).await
        }
        Some(path_str) => {
            let path_type = parse_path(&path_str)?;
            match path_type {
                PathType::S3 { bucket, key } => {
                    if versions {
                        if recursive {
                            eprintln!(
                                "Warning: --recursive is ignored when --versions is specified"
                            );
                        }
                        list_object_versions(client, &bucket, &key, human_readable).await
                    } else {
                        list_objects(client, &bucket, &key, recursive).await
                    }
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

/// List all versions and delete markers for a bucket/prefix
async fn list_object_versions(
    client: &Client,
    bucket: &str,
    prefix: &str,
    human_readable: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("KEY\tVERSION-ID\tLATEST\tTYPE\tLAST-MODIFIED\tSIZE");

    let mut key_marker: Option<String> = None;
    let mut version_id_marker: Option<String> = None;

    loop {
        let mut req = client.list_object_versions().bucket(bucket);

        if !prefix.is_empty() {
            req = req.prefix(prefix);
        }
        if let Some(ref km) = key_marker {
            req = req.key_marker(km);
        }
        if let Some(ref vim) = version_id_marker {
            req = req.version_id_marker(vim);
        }

        let resp = req.send().await?;

        for v in resp.versions() {
            let key = v.key().unwrap_or("");
            let version_id = v.version_id().unwrap_or("");
            let is_latest = v.is_latest().unwrap_or(false);
            let last_modified = v
                .last_modified()
                .and_then(|d| d.fmt(Format::DateTime).ok())
                .unwrap_or_else(|| "N/A".to_string());
            let size = v.size().unwrap_or(0);
            println!(
                "{}\t{}\t{}\tVersion\t{}\t{}",
                key,
                version_id,
                is_latest,
                last_modified,
                format_size(size, human_readable)
            );
        }

        for dm in resp.delete_markers() {
            let key = dm.key().unwrap_or("");
            let version_id = dm.version_id().unwrap_or("");
            let is_latest = dm.is_latest().unwrap_or(false);
            let last_modified = dm
                .last_modified()
                .and_then(|d| d.fmt(Format::DateTime).ok())
                .unwrap_or_else(|| "N/A".to_string());
            println!(
                "{}\t{}\t{}\tDeleteMarker\t{}\t-",
                key, version_id, is_latest, last_modified
            );
        }

        if resp.is_truncated() == Some(true) {
            key_marker = resp.next_key_marker().map(|s| s.to_string());
            version_id_marker = resp.next_version_id_marker().map(|s| s.to_string());
        } else {
            break;
        }
    }

    Ok(())
}

fn format_size(bytes: i64, human_readable: bool) -> String {
    if !human_readable {
        return bytes.to_string();
    }
    if bytes >= 1 << 30 {
        format!("{:.1}GB", bytes as f64 / (1u64 << 30) as f64)
    } else if bytes >= 1 << 20 {
        format!("{:.1}MB", bytes as f64 / (1u64 << 20) as f64)
    } else if bytes >= 1 << 10 {
        format!("{:.1}KB", bytes as f64 / (1u64 << 10) as f64)
    } else {
        format!("{}B", bytes)
    }
}

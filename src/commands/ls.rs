use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use aws_smithy_types::date_time::Format;
use serde::Serialize;

#[derive(Serialize)]
struct BucketEntry {
    name: String,
    creation_date: Option<String>,
}

#[derive(Serialize)]
struct ListBucketsOutput {
    buckets: Vec<BucketEntry>,
    total_buckets: usize,
}

#[derive(Serialize)]
struct PrefixEntry {
    key: String,
    kind: String,
}

#[derive(Serialize)]
struct ObjectEntry {
    key: String,
    last_modified: Option<String>,
    size: i64,
}

#[derive(Serialize)]
struct ListObjectsOutput {
    bucket: String,
    prefix: String,
    recursive: bool,
    prefixes: Vec<PrefixEntry>,
    objects: Vec<ObjectEntry>,
    total_objects: usize,
    total_size: i64,
}

#[derive(Serialize)]
struct VersionEntry {
    key: String,
    version_id: String,
    latest: bool,
    kind: String,
    last_modified: Option<String>,
    size: Option<i64>,
    size_display: Option<String>,
}

#[derive(Serialize)]
struct ListVersionsOutput {
    bucket: String,
    prefix: String,
    versions: Vec<VersionEntry>,
}

pub async fn list(
    client: &Client,
    path: Option<String>,
    recursive: bool,
    versions: bool,
    human_readable: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match path {
        None => {
            let output = list_buckets(client).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else if output.buckets.is_empty() {
                println!("No buckets found");
            } else {
                for bucket in &output.buckets {
                    println!(
                        "{:30} {}",
                        bucket.creation_date.as_deref().unwrap_or("N/A"),
                        bucket.name
                    );
                }
                println!("\nTotal buckets: {}", output.total_buckets);
            }
            Ok(())
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
                        let output =
                            list_object_versions(client, &bucket, &key, human_readable).await?;
                        if json {
                            println!("{}", serde_json::to_string_pretty(&output)?);
                        } else {
                            println!("KEY\tVERSION-ID\tLATEST\tTYPE\tLAST-MODIFIED\tSIZE");
                            for entry in &output.versions {
                                println!(
                                    "{}\t{}\t{}\t{}\t{}\t{}",
                                    entry.key,
                                    entry.version_id,
                                    entry.latest,
                                    entry.kind,
                                    entry.last_modified.as_deref().unwrap_or("N/A"),
                                    entry.size_display.as_deref().unwrap_or("-"),
                                );
                            }
                        }
                    } else {
                        let output = list_objects(client, &bucket, &key, recursive).await?;
                        if json {
                            println!("{}", serde_json::to_string_pretty(&output)?);
                        } else {
                            for prefix in &output.prefixes {
                                println!("{:>20} {}", "PRE", prefix.key);
                            }
                            for object in &output.objects {
                                println!(
                                    "{:30} {:>12} {}",
                                    object.last_modified.as_deref().unwrap_or("N/A"),
                                    object.size,
                                    object.key
                                );
                            }
                            println!(
                                "\nTotal objects: {}, Total size: {} bytes",
                                output.total_objects, output.total_size
                            );
                        }
                    }
                    Ok(())
                }
                PathType::Local(_) => {
                    Err("ls command requires S3 URI (s3://bucket[/prefix])".into())
                }
            }
        }
    }
}

async fn list_buckets(client: &Client) -> Result<ListBucketsOutput, Box<dyn std::error::Error>> {
    let response = client.list_buckets().send().await?;
    let buckets = response
        .buckets()
        .iter()
        .map(|bucket| BucketEntry {
            name: bucket.name().unwrap_or_default().to_string(),
            creation_date: bucket.creation_date().map(|d| d.to_string()),
        })
        .collect::<Vec<_>>();

    Ok(ListBucketsOutput {
        total_buckets: buckets.len(),
        buckets,
    })
}

async fn list_objects(
    client: &Client,
    bucket: &str,
    prefix: &str,
    recursive: bool,
) -> Result<ListObjectsOutput, Box<dyn std::error::Error>> {
    let mut continuation_token: Option<String> = None;
    let mut total_size = 0i64;
    let mut prefixes = Vec::new();
    let mut objects = Vec::new();

    loop {
        let mut request = client.list_objects_v2().bucket(bucket);
        if !prefix.is_empty() {
            request = request.prefix(prefix);
        }
        if !recursive {
            request = request.delimiter("/");
        }
        if let Some(token) = continuation_token {
            request = request.continuation_token(token);
        }

        let response = request.send().await?;

        if !recursive {
            for common_prefix in response.common_prefixes() {
                if let Some(prefix_str) = common_prefix.prefix() {
                    prefixes.push(PrefixEntry {
                        key: prefix_str.to_string(),
                        kind: "prefix".to_string(),
                    });
                }
            }
        }

        for obj in response.contents() {
            if let Some(key) = obj.key() {
                let size = obj.size().unwrap_or(0);
                total_size += size;
                objects.push(ObjectEntry {
                    key: key.to_string(),
                    last_modified: obj.last_modified().map(|d| d.to_string()),
                    size,
                });
            }
        }

        if response.is_truncated() == Some(true) {
            continuation_token = response.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    Ok(ListObjectsOutput {
        bucket: bucket.to_string(),
        prefix: prefix.to_string(),
        recursive,
        total_objects: objects.len(),
        total_size,
        prefixes,
        objects,
    })
}

async fn list_object_versions(
    client: &Client,
    bucket: &str,
    prefix: &str,
    human_readable: bool,
) -> Result<ListVersionsOutput, Box<dyn std::error::Error>> {
    let mut key_marker: Option<String> = None;
    let mut version_id_marker: Option<String> = None;
    let mut versions = Vec::new();

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
            let size = v.size().unwrap_or(0);
            versions.push(VersionEntry {
                key: v.key().unwrap_or_default().to_string(),
                version_id: v.version_id().unwrap_or_default().to_string(),
                latest: v.is_latest().unwrap_or(false),
                kind: "Version".to_string(),
                last_modified: v.last_modified().and_then(|d| d.fmt(Format::DateTime).ok()),
                size: Some(size),
                size_display: Some(format_size(size, human_readable)),
            });
        }

        for dm in resp.delete_markers() {
            versions.push(VersionEntry {
                key: dm.key().unwrap_or_default().to_string(),
                version_id: dm.version_id().unwrap_or_default().to_string(),
                latest: dm.is_latest().unwrap_or(false),
                kind: "DeleteMarker".to_string(),
                last_modified: dm
                    .last_modified()
                    .and_then(|d| d.fmt(Format::DateTime).ok()),
                size: None,
                size_display: Some("-".to_string()),
            });
        }

        if resp.is_truncated() == Some(true) {
            key_marker = resp.next_key_marker().map(|s| s.to_string());
            version_id_marker = resp.next_version_id_marker().map(|s| s.to_string());
        } else {
            break;
        }
    }

    Ok(ListVersionsOutput {
        bucket: bucket.to_string(),
        prefix: prefix.to_string(),
        versions,
    })
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

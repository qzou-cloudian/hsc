use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::types::ObjectAttributes;
use aws_sdk_s3::Client;
use serde::Serialize;

/// One part's metadata as reported by the server.
#[derive(Debug, Default, Serialize)]
struct PartEntry {
    part_number: Option<i32>,
    size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum_crc32: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum_crc32c: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum_crc64nvme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum_sha1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum_sha256: Option<String>,
}

/// Complete output structure for `hsc parts`.
#[derive(Debug, Serialize)]
struct PartsOutput {
    path: String,
    bucket: String,
    key: String,
    object_size: Option<i64>,
    etag: Option<String>,
    storage_class: Option<String>,
    total_parts_count: Option<i32>,
    is_truncated: bool,
    parts: Vec<PartEntry>,
}

pub async fn parts(
    client: &Client,
    path: &str,
    attributes: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let PathType::S3 { bucket, key } = parse_path(path)? else {
        return Err("parts command requires an S3 object path".into());
    };
    if key.is_empty() {
        return Err("parts command requires an S3 object path, not a bucket".into());
    }

    let output = if attributes {
        parts_via_get_object_attributes(client, path, &bucket, &key).await?
    } else {
        parts_via_head_object(client, path, &bucket, &key).await?
    };

    render_output(&output, json)?;
    Ok(())
}

/// Fetches part data via `GetObjectAttributes` (AWS-native; includes per-part checksums).
async fn parts_via_get_object_attributes(
    client: &Client,
    path: &str,
    bucket: &str,
    key: &str,
) -> Result<PartsOutput, Box<dyn std::error::Error>> {
    let mut all_parts = Vec::new();
    let mut marker: Option<String> = None;
    let mut total_parts_count = None;
    let mut object_size = None;
    let mut etag = None;
    let mut storage_class = None;
    let mut is_truncated = false;

    loop {
        let mut request = client
            .get_object_attributes()
            .bucket(bucket)
            .key(key)
            .object_attributes(ObjectAttributes::ObjectParts)
            .object_attributes(ObjectAttributes::ObjectSize)
            .object_attributes(ObjectAttributes::Etag)
            .object_attributes(ObjectAttributes::StorageClass)
            .max_parts(1000);

        if let Some(ref marker_value) = marker {
            request = request.part_number_marker(marker_value);
        }

        let response = request.send().await?;
        object_size = object_size.or(response.object_size());
        etag = etag.or_else(|| response.e_tag().map(str::to_string));
        storage_class =
            storage_class.or_else(|| response.storage_class().map(|v| v.as_str().to_string()));

        if let Some(object_parts) = response.object_parts() {
            total_parts_count = total_parts_count.or(object_parts.total_parts_count());
            is_truncated = object_parts.is_truncated().unwrap_or(false);

            for part in object_parts.parts() {
                all_parts.push(PartEntry {
                    part_number: part.part_number(),
                    size: part.size(),
                    checksum_crc32: part.checksum_crc32().map(str::to_string),
                    checksum_crc32c: part.checksum_crc32_c().map(str::to_string),
                    checksum_crc64nvme: part.checksum_crc64_nvme().map(str::to_string),
                    checksum_sha1: part.checksum_sha1().map(str::to_string),
                    checksum_sha256: part.checksum_sha256().map(str::to_string),
                });
            }

            marker = object_parts.next_part_number_marker().map(str::to_string);
            if !is_truncated {
                break;
            }
        } else {
            break;
        }
    }

    Ok(PartsOutput {
        path: path.to_string(),
        bucket: bucket.to_string(),
        key: key.to_string(),
        object_size,
        etag,
        storage_class,
        total_parts_count,
        is_truncated,
        parts: all_parts,
    })
}

/// Fetches part data via `HeadObject` requests (works on any S3-compatible server).
///
/// Issues one `HeadObject` for object-level metadata and one per part to determine
/// individual part sizes. Per-part checksums are not available via this path.
async fn parts_via_head_object(
    client: &Client,
    path: &str,
    bucket: &str,
    key: &str,
) -> Result<PartsOutput, Box<dyn std::error::Error>> {
    let head = client.head_object().bucket(bucket).key(key).send().await?;
    let object_size = head.content_length();
    let etag = head.e_tag().map(str::to_string);
    let storage_class = head.storage_class().map(|s| s.as_str().to_string());

    // Requesting part 1 reveals the total part count via x-amz-mp-parts-count.
    let part1 = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .part_number(1)
        .send()
        .await?;
    let total_parts = part1.parts_count();

    if total_parts.is_none() {
        // Single-put object — no multipart metadata available.
        return Ok(PartsOutput {
            path: path.to_string(),
            bucket: bucket.to_string(),
            key: key.to_string(),
            object_size,
            etag,
            storage_class,
            total_parts_count: None,
            is_truncated: false,
            parts: vec![],
        });
    }

    let total = total_parts.unwrap();
    let mut all_parts: Vec<PartEntry> = Vec::with_capacity(total as usize);

    // Reuse the part-1 response already in hand, then fetch the rest sequentially.
    all_parts.push(PartEntry {
        part_number: Some(1),
        size: part1.content_length(),
        ..Default::default()
    });

    for n in 2..=total {
        let resp = client
            .head_object()
            .bucket(bucket)
            .key(key)
            .part_number(n)
            .send()
            .await?;
        all_parts.push(PartEntry {
            part_number: Some(n),
            size: resp.content_length(),
            ..Default::default()
        });
    }

    Ok(PartsOutput {
        path: path.to_string(),
        bucket: bucket.to_string(),
        key: key.to_string(),
        object_size,
        etag,
        storage_class,
        total_parts_count: Some(total),
        is_truncated: false,
        parts: all_parts,
    })
}

fn render_output(output: &PartsOutput, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Path      : {}", output.path);
        if let Some(size) = output.object_size {
            println!("Size      : {} bytes", size);
        }
        if let Some(ref etag) = output.etag {
            println!("ETag      : {}", etag);
        }
        if let Some(ref sc) = output.storage_class {
            println!("Storage   : {}", sc);
        }
        match output.total_parts_count {
            Some(count) => println!("Parts     : {}", count),
            None => println!("Parts     : 1 (single-put)"),
        }
        if !output.parts.is_empty() {
            let has_checksums = output.parts.iter().any(|p| {
                p.checksum_crc32.is_some()
                    || p.checksum_crc32c.is_some()
                    || p.checksum_crc64nvme.is_some()
                    || p.checksum_sha1.is_some()
                    || p.checksum_sha256.is_some()
            });
            println!();
            if has_checksums {
                println!("PART\tSIZE\tCRC32\tCRC32C\tCRC64NVME\tSHA1\tSHA256");
                for part in &output.parts {
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        part.part_number
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        part.size
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        part.checksum_crc32.as_deref().unwrap_or("-"),
                        part.checksum_crc32c.as_deref().unwrap_or("-"),
                        part.checksum_crc64nvme.as_deref().unwrap_or("-"),
                        part.checksum_sha1.as_deref().unwrap_or("-"),
                        part.checksum_sha256.as_deref().unwrap_or("-"),
                    );
                }
            } else {
                println!("PART\tSIZE");
                for part in &output.parts {
                    println!(
                        "{}\t{}",
                        part.part_number
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        part.size
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                    );
                }
            }
        }
    }
    Ok(())
}

use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use aws_smithy_types::date_time::Format;

pub async fn list_versions(
    client: &Client,
    path: &str,
    human_readable: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let path_type = parse_path(path)?;

    let (bucket, prefix) = match path_type {
        PathType::S3 { bucket, key } => (bucket, key),
        PathType::Local(_) => {
            return Err(
                "versions command requires an S3 path (s3://bucket[/prefix])".into(),
            )
        }
    };

    println!("KEY\tVERSION-ID\tLATEST\tTYPE\tLAST-MODIFIED\tSIZE");

    let mut key_marker: Option<String> = None;
    let mut version_id_marker: Option<String> = None;

    loop {
        let mut req = client.list_object_versions().bucket(&bucket);

        if !prefix.is_empty() {
            req = req.prefix(&prefix);
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

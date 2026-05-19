use aws_sdk_s3::Client;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub(crate) struct S3ObjectEntry {
    pub key: String,
    pub size: i64,
    pub etag: Option<String>,
}

impl S3ObjectEntry {
    pub(crate) fn relative_key<'a>(&'a self, prefix: &str) -> &'a str {
        relative_s3_key(&self.key, prefix)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LocalFileEntry {
    pub path: PathBuf,
    pub relative: String,
    pub relative_unix: String,
}

pub(crate) async fn list_s3_objects(
    client: &Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<S3ObjectEntry>, Box<dyn std::error::Error>> {
    let mut objects = Vec::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let mut request = client.list_objects_v2().bucket(bucket);

        if !prefix.is_empty() {
            request = request.prefix(prefix);
        }

        if let Some(token) = continuation_token {
            request = request.continuation_token(token);
        }

        let response = request.send().await?;

        for obj in response.contents() {
            if let Some(key) = obj.key() {
                objects.push(S3ObjectEntry {
                    key: key.to_string(),
                    size: obj.size().unwrap_or(0),
                    etag: obj.e_tag().map(str::to_string),
                });
            }
        }

        if response.is_truncated() == Some(true) {
            continuation_token = response.next_continuation_token().map(str::to_string);
        } else {
            break;
        }
    }

    Ok(objects)
}

pub(crate) async fn list_s3_keys(
    client: &Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(list_s3_objects(client, bucket, prefix)
        .await?
        .into_iter()
        .map(|obj| obj.key)
        .collect())
}

pub(crate) async fn list_s3_keys_limited(
    client: &Client,
    bucket: &str,
    prefix: &str,
    limit: u64,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut keys = Vec::new();
    let mut continuation_token: Option<String> = None;

    while (keys.len() as u64) < limit {
        let remaining = limit - keys.len() as u64;
        let page_size = remaining.min(1000) as i32;

        let mut request = client.list_objects_v2().bucket(bucket).max_keys(page_size);
        if !prefix.is_empty() {
            request = request.prefix(prefix);
        }
        if let Some(token) = continuation_token.take() {
            request = request.continuation_token(token);
        }

        let response = request.send().await?;
        for obj in response.contents() {
            if let Some(key) = obj.key() {
                keys.push(key.to_string());
                if keys.len() as u64 >= limit {
                    return Ok(keys);
                }
            }
        }

        if response.is_truncated().unwrap_or(false) {
            continuation_token = response.next_continuation_token().map(str::to_string);
        } else {
            break;
        }
    }

    Ok(keys)
}

pub(crate) async fn s3_prefix_has_objects(
    client: &Client,
    bucket: &str,
    prefix: &str,
) -> Result<Option<bool>, Box<dyn std::error::Error>> {
    let mut request = client.list_objects_v2().bucket(bucket).max_keys(1);
    if !prefix.is_empty() {
        request = request.prefix(prefix);
    }

    let response = request.send().await?;
    Ok(response.key_count().map(|count| count > 0))
}

pub(crate) fn walk_local_files(
    base_path: &str,
) -> Result<Vec<LocalFileEntry>, Box<dyn std::error::Error>> {
    let base = Path::new(base_path);
    let mut entries = Vec::new();

    for entry in WalkDir::new(base).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let relative = path
            .strip_prefix(base)
            .map_err(|e| format!("Path error: {}", e))?
            .to_string_lossy()
            .to_string();
        let relative_unix = relative.replace('\\', "/");

        entries.push(LocalFileEntry {
            path: path.to_path_buf(),
            relative,
            relative_unix,
        });
    }

    Ok(entries)
}

pub(crate) fn relative_s3_key<'a>(key: &'a str, prefix: &str) -> &'a str {
    if !prefix.is_empty() && key.starts_with(prefix) {
        key[prefix.len()..].trim_start_matches('/')
    } else {
        key
    }
}

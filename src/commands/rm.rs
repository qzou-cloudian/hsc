use crate::filters::FileFilter;
use crate::path_utils::{parse_s3_uri, PathType};
use aws_sdk_s3::Client;

/// Remove S3 objects
pub async fn remove(
    client: &Client,
    path: &str,
    recursive: bool,
    include: Vec<String>,
    exclude: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path_type = parse_s3_uri(path)?;

    let (bucket, key) = match path_type {
        PathType::S3 { bucket, key } => (bucket, key),
        PathType::Local(_) => {
            return Err("rm command requires S3 URI (s3://bucket/key)".into());
        }
    };

    if recursive {
        let filter = FileFilter::new(include, exclude)?;
        remove_recursive(client, &bucket, &key, &filter).await
    } else {
        remove_single(client, &bucket, &key).await
    }
}

/// Remove a single S3 object
async fn remove_single(
    client: &Client,
    bucket: &str,
    key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if key.is_empty() {
        return Err(
            "Key is required for single object removal. Use rb command to remove buckets.".into(),
        );
    }

    client
        .delete_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await?;

    println!("Deleted: s3://{}/{}", bucket, key);
    Ok(())
}

/// Remove objects recursively with optional filters
async fn remove_recursive(
    client: &Client,
    bucket: &str,
    prefix: &str,
    filter: &FileFilter,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut continuation_token: Option<String> = None;
    let mut deleted_count = 0;

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
                // Apply filters
                if !filter.matches(key) {
                    continue;
                }

                client
                    .delete_object()
                    .bucket(bucket)
                    .key(key)
                    .send()
                    .await?;

                println!("Deleted: s3://{}/{}", bucket, key);
                deleted_count += 1;
            }
        }

        if response.is_truncated() == Some(true) {
            continuation_token = response.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    println!("Total deleted: {} objects", deleted_count);
    Ok(())
}

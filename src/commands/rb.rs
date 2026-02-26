use crate::path_utils::{parse_s3_uri, PathType};
use aws_sdk_s3::Client;

/// Remove an S3 bucket
pub async fn remove_bucket(
    client: &Client,
    bucket_uri: &str,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = parse_s3_uri(bucket_uri)?;

    let bucket_name = match path {
        PathType::S3 { bucket, key } => {
            if !key.is_empty() {
                return Err(format!(
                    "rb command expects bucket URI only (s3://bucket-name), got key: {}",
                    key
                )
                .into());
            }
            bucket
        }
        PathType::Local(_) => {
            return Err("rb command requires S3 URI (s3://bucket-name)".into());
        }
    };

    // Check if bucket is empty unless force flag is set
    if !force {
        let objects = client
            .list_objects_v2()
            .bucket(&bucket_name)
            .max_keys(1)
            .send()
            .await?;

        if !objects.contents().is_empty() {
            return Err(format!(
                "Bucket '{}' is not empty. Use --force to delete non-empty bucket",
                bucket_name
            )
            .into());
        }
    } else {
        // Delete all objects in the bucket first
        println!("Force flag enabled, deleting all objects in bucket...");
        delete_all_objects(client, &bucket_name).await?;
    }

    println!("Deleting bucket: {}", bucket_name);

    client.delete_bucket().bucket(&bucket_name).send().await?;

    println!("Successfully deleted bucket: {}", bucket_name);
    Ok(())
}

/// Delete all objects in a bucket
async fn delete_all_objects(
    client: &Client,
    bucket: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut continuation_token: Option<String> = None;

    loop {
        let mut request = client.list_objects_v2().bucket(bucket);

        if let Some(token) = continuation_token {
            request = request.continuation_token(token);
        }

        let response = request.send().await?;

        let objects = response.contents();
        if objects.is_empty() {
            break;
        }

        for obj in objects {
            if let Some(key) = obj.key() {
                client
                    .delete_object()
                    .bucket(bucket)
                    .key(key)
                    .send()
                    .await?;
                println!("Deleted: {}", key);
            }
        }

        if response.is_truncated() == Some(true) {
            continuation_token = response.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    Ok(())
}

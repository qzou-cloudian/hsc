use crate::path_utils::{parse_s3_uri, PathType};
use aws_sdk_s3::Client;

/// Create an S3 bucket
pub async fn make_bucket(
    client: &Client,
    bucket_uri: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = parse_s3_uri(bucket_uri)?;

    let bucket_name = match path {
        PathType::S3 { bucket, key } => {
            if !key.is_empty() {
                return Err(format!(
                    "mb command expects bucket URI only (s3://bucket-name), got key: {}",
                    key
                )
                .into());
            }
            bucket
        }
        PathType::Local(_) => {
            return Err("mb command requires S3 URI (s3://bucket-name)".into());
        }
    };

    println!("Creating bucket: {}", bucket_name);

    client.create_bucket().bucket(&bucket_name).send().await?;

    println!("Successfully created bucket: {}", bucket_name);
    Ok(())
}

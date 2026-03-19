use crate::path_utils::{parse_s3_uri, PathType};
use aws_sdk_s3::operation::create_bucket::CreateBucketError;
use aws_sdk_s3::Client;
use aws_smithy_runtime_api::client::result::SdkError;

/// Create an S3 bucket
pub async fn make_bucket(
    client: &Client,
    bucket_uri: &str,
    ignore_existing: bool,
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

    let result = client.create_bucket().bucket(&bucket_name).send().await;

    if let Err(SdkError::ServiceError(ref e)) = result {
        if ignore_existing {
            match e.err() {
                CreateBucketError::BucketAlreadyExists(_)
                | CreateBucketError::BucketAlreadyOwnedByYou(_) => {
                    println!("Bucket already exists (ignored): {}", bucket_name);
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    result?;
    println!("Successfully created bucket: {}", bucket_name);
    Ok(())
}

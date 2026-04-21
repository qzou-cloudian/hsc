use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;
use serde::Serialize;

#[derive(Serialize)]
struct ExistsOutput {
    path: String,
    exists: bool,
}

pub async fn exists(
    client: &Client,
    path: &str,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let exists = match parse_path(path)? {
        PathType::Local(local_path) => std::path::Path::new(&local_path).exists(),
        PathType::S3 { bucket, key } => {
            if key.is_empty() {
                match client.head_bucket().bucket(bucket).send().await {
                    Ok(_) => true,
                    Err(err) if is_not_found(&err) => false,
                    Err(err) => return Err(err.into()),
                }
            } else {
                match client.head_object().bucket(bucket).key(key).send().await {
                    Ok(_) => true,
                    Err(err) if is_not_found(&err) => false,
                    Err(err) => return Err(err.into()),
                }
            }
        }
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ExistsOutput {
                path: path.to_string(),
                exists,
            })?
        );
    } else {
        println!("{}", if exists { "true" } else { "false" });
    }

    if exists {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn is_not_found<E>(err: &E) -> bool
where
    E: std::fmt::Display + ProvideErrorMetadata,
{
    let code = err.code().unwrap_or_default();
    let message = err.to_string();
    matches!(code, "404" | "NotFound" | "NoSuchKey" | "NoSuchBucket")
        || message.contains("Not Found")
        || message.contains("status: 404")
        || message.contains("NoSuchKey")
        || message.contains("NoSuchBucket")
}

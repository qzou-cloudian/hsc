use crate::commands::cp;
use crate::commands::rm;
use aws_sdk_s3::Client;

/// Move files (copy + delete source)
pub async fn move_files(
    client: &Client,
    source: &str,
    dest: &str,
    recursive: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    multipart_threshold: u64,
    multipart_chunksize: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    // First, copy the files
    cp::copy(
        client,
        source,
        dest,
        recursive,
        include.clone(),
        exclude.clone(),
        None, // No checksum for move operations
        None,
        multipart_threshold,
        multipart_chunksize,
    )
    .await?;

    // Then, delete the source
    // Only delete from S3 (moving from local would delete local files)
    if source.starts_with("s3://") {
        println!("\nRemoving source files...");
        rm::remove(client, source, recursive, include, exclude).await?;
    } else {
        println!("Note: Source files in local filesystem were not removed");
    }

    Ok(())
}

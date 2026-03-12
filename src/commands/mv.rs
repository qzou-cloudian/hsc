use crate::commands::cp;
use crate::commands::rm;
use aws_sdk_s3::Client;

#[cfg(feature = "rdma")]
use crate::rdma::RdmaProvider;
#[cfg(feature = "rdma")]
use std::sync::Arc;

/// Move files (copy + delete source)
#[allow(clippy::too_many_arguments)]
pub async fn move_files(
    client: &Client,
    source: &str,
    dest: &str,
    recursive: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    multipart_threshold: u64,
    multipart_chunksize: u64,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
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
        multipart_threshold,
        multipart_chunksize,
        #[cfg(feature = "rdma")] rdma,
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

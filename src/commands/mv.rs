use crate::commands::cp;
use crate::commands::rm;
use crate::commands::transfer::{MultipartConfig, SseConfig};
use aws_sdk_s3::Client;

#[cfg(feature = "rdma")]
use crate::rdma::RdmaClientProvider;
#[cfg(feature = "rdma")]
use std::sync::Arc;

/// Move files (copy + delete source)
pub struct MoveOptions {
    pub recursive: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub sse: SseConfig,
    pub multipart: MultipartConfig,
    #[cfg(feature = "rdma")]
    pub rdma: Option<Arc<dyn RdmaClientProvider>>,
}

pub async fn move_files(
    client: &Client,
    source: &str,
    dest: &str,
    opts: MoveOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    // First, copy the files
    cp::copy(
        client,
        source,
        dest,
        cp::CopyOptions {
            recursive: opts.recursive,
            include: opts.include.clone(),
            exclude: opts.exclude.clone(),
            checksum: None, // No checksum for move operations
            sse: opts.sse,
            multipart: opts.multipart,
            #[cfg(feature = "rdma")]
            rdma: opts.rdma,
        },
    )
    .await?;

    // Then, delete the source
    // Only delete from S3 (moving from local would delete local files)
    if source.starts_with("s3://") {
        println!("\nRemoving source files...");
        rm::remove(client, source, opts.recursive, opts.include, opts.exclude).await?;
    } else {
        println!("Note: Source files in local filesystem were not removed");
    }

    Ok(())
}

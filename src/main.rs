use clap::{Parser, Subcommand};

mod commands;
mod filters;
mod path_utils;
mod s3_client;

#[derive(Parser)]
#[command(name = "hsc")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "AWS S3 CLI tool", long_about = None)]
struct Cli {
    /// Enable debug output
    #[arg(long, global = true)]
    debug: bool,

    /// Override the S3 endpoint URL
    #[arg(long, global = true)]
    endpoint_url: Option<String>,

    /// Disable SSL certificate verification (use with caution)
    #[arg(long, global = true)]
    no_verify_ssl: bool,

    /// Use a specific AWS profile from credentials file
    #[arg(long, global = true)]
    profile: Option<String>,

    /// AWS region to use
    #[arg(long, global = true)]
    region: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create an S3 bucket
    Mb {
        /// S3 URI (s3://bucket-name)
        bucket: String,
    },
    /// Remove an S3 bucket
    Rb {
        /// S3 URI (s3://bucket-name)
        bucket: String,
        /// Force removal even if bucket is not empty
        #[arg(long)]
        force: bool,
    },
    /// List S3 buckets or objects
    Ls {
        /// S3 URI (s3://bucket-name/prefix or empty for all buckets)
        path: Option<String>,
        /// List all objects recursively
        #[arg(long)]
        recursive: bool,
    },
    /// Copy files
    Cp {
        /// Source path (local path or s3://bucket/key)
        source: String,
        /// Destination path (local path or s3://bucket/key)
        dest: String,
        /// Copy directories recursively
        #[arg(long)]
        recursive: bool,
        /// Include files matching pattern (can be specified multiple times)
        #[arg(long)]
        include: Vec<String>,
        /// Exclude files matching pattern (can be specified multiple times)
        #[arg(long)]
        exclude: Vec<String>,
        /// Checksum mode (ENABLED for single object operations)
        #[arg(long)]
        checksum_mode: Option<String>,
        /// Checksum algorithm (CRC32, CRC32C, SHA1, SHA256)
        #[arg(long)]
        checksum_algorithm: Option<String>,
    },
    /// Synchronize directories
    Sync {
        /// Source path (local path or s3://bucket/prefix)
        source: String,
        /// Destination path (local path or s3://bucket/prefix)
        dest: String,
        /// Include files matching pattern (can be specified multiple times)
        #[arg(long)]
        include: Vec<String>,
        /// Exclude files matching pattern (can be specified multiple times)
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// Move files
    Mv {
        /// Source path (local path or s3://bucket/key)
        source: String,
        /// Destination path (local path or s3://bucket/key)
        dest: String,
        /// Move directories recursively
        #[arg(long)]
        recursive: bool,
        /// Include files matching pattern (can be specified multiple times)
        #[arg(long)]
        include: Vec<String>,
        /// Exclude files matching pattern (can be specified multiple times)
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// Remove S3 objects
    Rm {
        /// S3 URI (s3://bucket/key)
        path: String,
        /// Remove objects recursively
        #[arg(long)]
        recursive: bool,
        /// Include files matching pattern (can be specified multiple times)
        #[arg(long)]
        include: Vec<String>,
        /// Exclude files matching pattern (can be specified multiple times)
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// Display file or object information
    Stat {
        /// Path (local path or s3://bucket/key or s3://bucket)
        path: String,
        /// Stat objects recursively
        #[arg(long)]
        recursive: bool,
        /// Checksum mode (ENABLED for local files)
        #[arg(long)]
        checksum_mode: Option<String>,
        /// Checksum algorithm (CRC32, CRC32C, SHA1, SHA256)
        #[arg(long)]
        checksum_algorithm: Option<String>,
    },
    /// Compare directories or buckets and show differences
    Diff {
        /// Source path (local path or s3://bucket/prefix)
        source: String,
        /// Destination path (local path or s3://bucket/prefix)
        dest: String,
        /// Compare object contents using ETag/checksums (slower)
        #[arg(long)]
        compare_content: bool,
        /// Include files matching pattern (can be specified multiple times)
        #[arg(long)]
        include: Vec<String>,
        /// Exclude files matching pattern (can be specified multiple times)
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// Concatenate and print file or object content to STDOUT
    Cat {
        /// Path (local path or s3://bucket/key)
        path: String,
        /// Byte range to read (e.g., "0-100" or "bytes=0-100")
        #[arg(long)]
        range: Option<String>,
        /// Offset to start reading from (bytes)
        #[arg(long)]
        offset: Option<u64>,
        /// Number of bytes to read (used with --offset)
        #[arg(long)]
        size: Option<u64>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Initialize S3 client with global options
    let client_config = s3_client::S3ClientConfig {
        endpoint_url: cli.endpoint_url,
        region: cli.region,
        profile: cli.profile,
        verify_ssl: !cli.no_verify_ssl,
        debug: cli.debug,
        multipart_threshold: 8388608, // Will be loaded from config
        multipart_chunksize: 8388608, // Will be loaded from config
    };

    let client_config_clone = client_config.clone();
    let client = s3_client::create_s3_client(client_config).await?;

    match cli.command {
        Commands::Mb { bucket } => commands::mb::make_bucket(&client, &bucket).await,
        Commands::Rb { bucket, force } => {
            commands::rb::remove_bucket(&client, &bucket, force).await
        }
        Commands::Ls { path, recursive } => commands::ls::list(&client, path, recursive).await,
        Commands::Cp {
            source,
            dest,
            recursive,
            include,
            exclude,
            checksum_mode,
            checksum_algorithm,
        } => {
            commands::cp::copy(
                &client,
                &source,
                &dest,
                recursive,
                include,
                exclude,
                checksum_mode,
                checksum_algorithm,
                client_config_clone.multipart_threshold,
                client_config_clone.multipart_chunksize,
            )
            .await
        }
        Commands::Sync {
            source,
            dest,
            include,
            exclude,
        } => {
            commands::sync::sync(
                &client,
                &source,
                &dest,
                include,
                exclude,
                client_config_clone.multipart_threshold,
                client_config_clone.multipart_chunksize,
            )
            .await
        }
        Commands::Mv {
            source,
            dest,
            recursive,
            include,
            exclude,
        } => {
            commands::mv::move_files(
                &client,
                &source,
                &dest,
                recursive,
                include,
                exclude,
                client_config_clone.multipart_threshold,
                client_config_clone.multipart_chunksize,
            )
            .await
        }
        Commands::Rm {
            path,
            recursive,
            include,
            exclude,
        } => commands::rm::remove(&client, &path, recursive, include, exclude).await,
        Commands::Stat {
            path,
            recursive,
            checksum_mode,
            checksum_algorithm,
        } => {
            commands::stat::stat(&client, &path, recursive, checksum_mode, checksum_algorithm).await
        }
        Commands::Diff {
            source,
            dest,
            compare_content,
            include,
            exclude,
        } => commands::diff::diff(&client, &source, &dest, compare_content, include, exclude).await,
        Commands::Cat {
            path,
            range,
            offset,
            size,
        } => commands::cat::cat(&client, &path, range, offset, size).await,
    }
}

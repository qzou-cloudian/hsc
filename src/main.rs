use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

mod commands;
mod debug_interceptor;
mod filters;
mod path_utils;
#[cfg(feature = "rdma")]
mod rdma;
mod s3_client;

#[derive(Parser)]
#[command(name = "hsc")]
#[command(about = "High-performance S3 Client", long_about = None)]
struct Cli {
    /// Enable debug output
    #[arg(long, global = true)]
    debug: bool,

    /// Enable RDMA transfers.  PROVIDER may be `cuobj` or `mock`.
    /// Omitting PROVIDER (bare `--rdma`) auto-selects the best available provider.
    #[cfg(feature = "rdma")]
    #[arg(long, global = true, num_args = 0..=1, value_name = "PROVIDER",
          default_missing_value = "auto")]
    rdma: Option<String>,

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
        /// Checksum algorithm (ENABLED, CRC32, CRC32C, SHA1, SHA256); bare --checksum enables with default algorithm
        #[arg(long, num_args = 0..=1, default_missing_value = "ENABLED")]
        checksum: Option<String>,
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
        /// Checksum algorithm (ENABLED, CRC32, CRC32C, SHA1, SHA256); bare --checksum enables with default algorithm
        #[arg(long, num_args = 0..=1, default_missing_value = "ENABLED")]
        checksum: Option<String>,
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
    /// Compare two files or objects byte-by-byte
    Cmp {
        /// First path (local path or s3://bucket/key)
        path1: String,
        /// Second path (local path or s3://bucket/key)
        path2: String,
        /// Byte range to compare (e.g., "0-999" or "bytes=0-999")
        #[arg(long)]
        range: Option<String>,
        /// Byte offset to start comparison from
        #[arg(long)]
        offset: Option<u64>,
        /// Number of bytes to compare (used with --offset)
        #[arg(long)]
        size: Option<u64>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build rich version string and inject into clap before parsing.
    let version: &'static str = {
        let base = env!("CARGO_PKG_VERSION");
        let s = {
            #[cfg(feature = "rdma")]
            { format!("{base}\n{}", rdma::rdma_provider_info()) }
            #[cfg(not(feature = "rdma"))]
            { format!("{base}\nRDMA providers: none (not built)") }
        };
        Box::leak(s.into_boxed_str())
    };
    let args: Vec<_> = std::env::args_os().collect();
    let matches = Cli::command().version(version).get_matches_from(args);
    let cli = Cli::from_arg_matches(&matches)?;

    // --debug flag or HSC_DEBUG env var enables the DebugInterceptor which
    // prints request/response headers for every S3 call.
    let debug = cli.debug
        || std::env::var("HSC_DEBUG")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

    // Initialize S3 client with global options
    let mut client_config = s3_client::S3ClientConfig {
        endpoint_url: cli.endpoint_url,
        region: cli.region,
        profile: cli.profile,
        verify_ssl: !cli.no_verify_ssl,
        debug,
        multipart_threshold: 8388608, // Will be loaded from config
        multipart_chunksize: 8388608, // Will be loaded from config
        #[cfg(feature = "rdma")]
        rdma_provider: cli.rdma.clone(),
        #[cfg(not(feature = "rdma"))]
        rdma_provider: None,
    };

    // Resolve RDMA settings from env/config before cloning so the provider
    // creation below sees the correct values.
    s3_client::resolve_rdma_settings(&mut client_config);

    let client_config_clone = client_config.clone();
    let client = s3_client::create_s3_client(client_config).await?;

    // Create an RDMA provider once if enabled; passed to every command that
    // performs getObject / putObject / uploadPart.
    #[cfg(feature = "rdma")]
    let rdma_provider: Option<std::sync::Arc<dyn rdma::RdmaProvider>> =
        match &client_config_clone.rdma_provider {
            Some(p) => Some(rdma::create_provider(p, client_config_clone.debug)?),
            None => None,
        };

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
            checksum,
        } => {
            commands::cp::copy(
                &client,
                &source,
                &dest,
                recursive,
                include,
                exclude,
                checksum,
                client_config_clone.multipart_threshold,
                client_config_clone.multipart_chunksize,
                #[cfg(feature = "rdma")] rdma_provider,
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
                #[cfg(feature = "rdma")] rdma_provider,
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
                #[cfg(feature = "rdma")] rdma_provider,
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
            checksum,
        } => {
            commands::stat::stat(&client, &path, recursive, checksum).await
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
        } => commands::cat::cat(
            &client,
            &path,
            range,
            offset,
            size,
            #[cfg(feature = "rdma")] rdma_provider,
        )
        .await,
        Commands::Cmp {
            path1,
            path2,
            range,
            offset,
            size,
        } => commands::cmp::cmp(
            &client,
            &path1,
            &path2,
            range,
            offset,
            size,
            #[cfg(feature = "rdma")] rdma_provider,
        )
        .await,
    }
}

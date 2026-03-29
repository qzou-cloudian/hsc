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

    /// Maximum time in seconds allowed for socket read operations (0 = no timeout)
    #[arg(long, global = true, value_name = "SECONDS")]
    cli_read_timeout: Option<u64>,

    /// Maximum time in seconds allowed for socket connection (0 = no timeout)
    #[arg(long, global = true, value_name = "SECONDS")]
    cli_connect_timeout: Option<u64>,

    /// Add a custom HTTP header to every request in KEY:VALUE format (can be specified multiple times)
    #[arg(short = 'H', long, global = true, value_name = "KEY:VALUE")]
    custom_header: Vec<String>,

    /// Disable AWS request signing (for public buckets or anonymous access)
    #[arg(long, global = true)]
    no_sign_request: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create an S3 bucket
    Mb {
        /// S3 URI (s3://bucket-name)
        bucket: String,
        /// Do not fail if the bucket already exists
        #[arg(long)]
        ignore_existing: bool,
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
        /// Server-side encryption algorithm for destination (AES256, aws:kms, aws:kms:dsse)
        #[arg(long, value_name = "ALGORITHM")]
        sse: Option<String>,
        /// KMS key ARN or alias (used with --sse aws:kms or aws:kms:dsse)
        #[arg(long, value_name = "KEY_ID")]
        sse_kms_key_id: Option<String>,
        /// SSE-C algorithm for destination (AES256)
        #[arg(long, value_name = "ALGORITHM")]
        sse_c: Option<String>,
        /// Base64-encoded 256-bit customer key for SSE-C destination encryption/decryption
        #[arg(long, value_name = "KEY")]
        sse_c_key: Option<String>,
        /// SSE-C algorithm for the copy source (S3-to-S3 copies only)
        #[arg(long, value_name = "ALGORITHM")]
        sse_c_copy_source: Option<String>,
        /// Base64-encoded 256-bit customer key for SSE-C copy source decryption
        #[arg(long, value_name = "KEY")]
        sse_c_copy_source_key: Option<String>,
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
        /// Checksum algorithm (ENABLED, CRC32, CRC32C, SHA1, SHA256); bare --checksum enables verification
        #[arg(long, num_args = 0..=1, default_missing_value = "ENABLED")]
        checksum: Option<String>,
        /// Delete destination objects/files that are not present in the source
        #[arg(long)]
        delete: bool,
        /// Server-side encryption algorithm for destination (AES256, aws:kms, aws:kms:dsse)
        #[arg(long, value_name = "ALGORITHM")]
        sse: Option<String>,
        /// KMS key ARN or alias (used with --sse aws:kms or aws:kms:dsse)
        #[arg(long, value_name = "KEY_ID")]
        sse_kms_key_id: Option<String>,
        /// SSE-C algorithm for destination (AES256)
        #[arg(long, value_name = "ALGORITHM")]
        sse_c: Option<String>,
        /// Base64-encoded 256-bit customer key for SSE-C destination encryption/decryption
        #[arg(long, value_name = "KEY")]
        sse_c_key: Option<String>,
        /// SSE-C algorithm for the copy source (S3-to-S3 copies only)
        #[arg(long, value_name = "ALGORITHM")]
        sse_c_copy_source: Option<String>,
        /// Base64-encoded 256-bit customer key for SSE-C copy source decryption
        #[arg(long, value_name = "KEY")]
        sse_c_copy_source_key: Option<String>,
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
        /// Server-side encryption algorithm for destination (AES256, aws:kms, aws:kms:dsse)
        #[arg(long, value_name = "ALGORITHM")]
        sse: Option<String>,
        /// KMS key ARN or alias (used with --sse aws:kms or aws:kms:dsse)
        #[arg(long, value_name = "KEY_ID")]
        sse_kms_key_id: Option<String>,
        /// SSE-C algorithm for destination (AES256)
        #[arg(long, value_name = "ALGORITHM")]
        sse_c: Option<String>,
        /// Base64-encoded 256-bit customer key for SSE-C destination encryption/decryption
        #[arg(long, value_name = "KEY")]
        sse_c_key: Option<String>,
        /// SSE-C algorithm for the copy source (S3-to-S3 copies only)
        #[arg(long, value_name = "ALGORITHM")]
        sse_c_copy_source: Option<String>,
        /// Base64-encoded 256-bit customer key for SSE-C copy source decryption
        #[arg(long, value_name = "KEY")]
        sse_c_copy_source_key: Option<String>,
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
        /// Download a specific part of a multipart-uploaded object (1-based)
        #[arg(long)]
        part_number: Option<i32>,
        /// Return a specific version of the object (S3 versioning)
        #[arg(long)]
        version_id: Option<String>,
    },
    /// List object versions in an S3 bucket or for a specific key
    Versions {
        /// S3 URI (s3://bucket[/prefix])
        path: String,
        /// Format sizes in human-readable units (KB, MB, GB)
        #[arg(long)]
        human_readable: bool,
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
            {
                format!("{base}\n{}", rdma::rdma_provider_info())
            }
            #[cfg(not(feature = "rdma"))]
            {
                format!("{base}\nRDMA providers: none (not built)")
            }
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
        read_timeout_secs: cli.cli_read_timeout,
        connect_timeout_secs: cli.cli_connect_timeout,
        custom_headers: cli.custom_header,
        no_sign_request: cli.no_sign_request,
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
        Commands::Mb {
            bucket,
            ignore_existing,
        } => commands::mb::make_bucket(&client, &bucket, ignore_existing).await,
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
            sse,
            sse_kms_key_id,
            sse_c,
            sse_c_key,
            sse_c_copy_source,
            sse_c_copy_source_key,
        } => {
            let sse_config = commands::cp::SseConfig {
                sse,
                sse_kms_key_id,
                sse_c,
                sse_c_key,
                sse_c_copy_source,
                sse_c_copy_source_key,
            };
            commands::cp::copy(
                &client,
                &source,
                &dest,
                recursive,
                include,
                exclude,
                checksum,
                sse_config,
                client_config_clone.multipart_threshold,
                client_config_clone.multipart_chunksize,
                #[cfg(feature = "rdma")]
                rdma_provider,
            )
            .await
        }
        Commands::Sync {
            source,
            dest,
            include,
            exclude,
            checksum,
            delete,
            sse,
            sse_kms_key_id,
            sse_c,
            sse_c_key,
            sse_c_copy_source,
            sse_c_copy_source_key,
        } => {
            let sse_config = commands::cp::SseConfig {
                sse,
                sse_kms_key_id,
                sse_c,
                sse_c_key,
                sse_c_copy_source,
                sse_c_copy_source_key,
            };
            commands::sync::sync(
                &client,
                &source,
                &dest,
                include,
                exclude,
                checksum,
                delete,
                sse_config,
                client_config_clone.multipart_threshold,
                client_config_clone.multipart_chunksize,
                #[cfg(feature = "rdma")]
                rdma_provider,
            )
            .await
        }
        Commands::Mv {
            source,
            dest,
            recursive,
            include,
            exclude,
            sse,
            sse_kms_key_id,
            sse_c,
            sse_c_key,
            sse_c_copy_source,
            sse_c_copy_source_key,
        } => {
            let sse_config = commands::cp::SseConfig {
                sse,
                sse_kms_key_id,
                sse_c,
                sse_c_key,
                sse_c_copy_source,
                sse_c_copy_source_key,
            };
            commands::mv::move_files(
                &client,
                &source,
                &dest,
                recursive,
                include,
                exclude,
                sse_config,
                client_config_clone.multipart_threshold,
                client_config_clone.multipart_chunksize,
                #[cfg(feature = "rdma")]
                rdma_provider,
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
        } => commands::stat::stat(&client, &path, recursive, checksum).await,
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
            part_number,
            version_id,
        } => {
            commands::cat::cat(
                &client,
                &path,
                range,
                offset,
                size,
                part_number,
                version_id,
                #[cfg(feature = "rdma")]
                rdma_provider,
            )
            .await
        }
        Commands::Versions { path, human_readable } => {
            commands::versions::list_versions(&client, &path, human_readable).await
        }
        Commands::Cmp {
            path1,
            path2,
            range,
            offset,
            size,
        } => {
            commands::cmp::cmp(
                &client,
                &path1,
                &path2,
                range,
                offset,
                size,
                #[cfg(feature = "rdma")]
                rdma_provider,
            )
            .await
        }
    }
}

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

mod commands;
mod debug_interceptor;
mod filters;
mod path_utils;
#[cfg(feature = "rdma")]
mod rdma;
mod redirect_interceptor;
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
        /// List all object versions and delete markers (versioned buckets only)
        #[arg(long)]
        versions: bool,
        /// Format sizes in human-readable units (KB, MB, GB); used with --versions
        #[arg(long)]
        human_readable: bool,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
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
        /// Disable multipart uploads; always use a single PUT (max 5 GiB per object)
        #[arg(long, conflicts_with = "part_size")]
        disable_multipart: bool,
        /// Part size for multipart uploads, e.g. 16m, 256m, 1g (sets both threshold and chunk size)
        #[arg(long, value_name = "SIZE", conflicts_with = "disable_multipart")]
        part_size: Option<String>,
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
        /// Disable multipart uploads; always use a single PUT (max 5 GiB per object)
        #[arg(long, conflicts_with = "part_size")]
        disable_multipart: bool,
        /// Part size for multipart uploads, e.g. 16m, 256m, 1g (sets both threshold and chunk size)
        #[arg(long, value_name = "SIZE", conflicts_with = "disable_multipart")]
        part_size: Option<String>,
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
        /// Disable multipart uploads; always use a single PUT (max 5 GiB per object)
        #[arg(long, conflicts_with = "part_size")]
        disable_multipart: bool,
        /// Part size for multipart uploads, e.g. 16m, 256m, 1g (sets both threshold and chunk size)
        #[arg(long, value_name = "SIZE", conflicts_with = "disable_multipart")]
        part_size: Option<String>,
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
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
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
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
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
    /// Compare two files or objects byte-by-byte; prints a content hash on match
    Cmp {
        /// First path (local path or s3://bucket/key)
        path1: String,
        /// Second path (local path or s3://bucket/key)
        path2: String,
        /// Hash algorithm to compute when content matches (MD5, CRC32, CRC32C, SHA1, SHA256)
        #[arg(long, default_value = "SHA256", value_name = "ALGORITHM")]
        algorithm: String,
        /// Byte range to compare (e.g., "0-999" or "bytes=0-999"); hash is skipped when set
        #[arg(long)]
        range: Option<String>,
        /// Byte offset to start comparison from
        #[arg(long)]
        offset: Option<u64>,
        /// Number of bytes to compare (used with --offset)
        #[arg(long)]
        size: Option<u64>,
        /// SSE-C algorithm for S3 objects (AES256)
        #[arg(long)]
        sse_c: Option<String>,
        /// SSE-C customer key (base64-encoded 32-byte AES key)
        #[arg(long)]
        sse_c_key: Option<String>,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },
    /// Test whether a local path or S3 bucket/object exists
    Exists {
        /// Path (local path or s3://bucket[/key])
        path: String,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },
    /// Compute a digest for a local file or S3 object
    Hash {
        /// Path (local file or s3://bucket/key)
        path: String,
        /// Hash algorithm (MD5, CRC32, CRC32C, SHA1, SHA256)
        #[arg(long, default_value = "SHA256", value_name = "ALGORITHM")]
        algorithm: String,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },
    /// Show multipart/object-part metadata for an S3 object
    Parts {
        /// S3 URI (s3://bucket/key)
        path: String,
        /// Use GetObjectAttributes API instead of HeadObject (AWS-only; provides per-part checksums)
        #[arg(long)]
        attributes: bool,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },
    /// Run functional tests against an S3 bucket/object
    Test {
        #[command(subcommand)]
        subcommand: TestSubcommand,
    },
    /// Run performance benchmarks against an S3 bucket
    Perf {
        #[command(subcommand)]
        subcommand: PerfSubcommand,
    },
}

/// Subcommands for `hsc test`
#[derive(Subcommand)]
enum TestSubcommand {
    /// Upload an object and verify it by comparing byte ranges against known boundaries.
    ///
    /// Generates (or uses) a local file, uploads it to S3, then runs whole-object
    /// and targeted range comparisons covering multipart part boundaries, server
    /// chunk boundaries, and EC stripe boundaries (C/4 and C/2 within each chunk).
    Object {
        /// S3 bucket name
        bucket: String,
        /// S3 object key (default: auto-generated)
        key: Option<String>,
        /// Local file to upload and compare (mutually exclusive with --bytes)
        #[arg(short = 'f', long, value_name = "PATH")]
        file: Option<String>,
        /// Generate a random local file of this size (e.g. 6m, 10m)
        #[arg(short = 'b', long, value_name = "SIZE", value_parser = parse_size)]
        bytes: Option<u64>,
        /// Server storage chunk size (default: 4m)
        #[arg(long, default_value = "4m", value_name = "SIZE", value_parser = parse_size)]
        chunk_size: u64,
        /// Multipart upload part size / threshold (default: 8m)
        #[arg(long, default_value = "8m", value_name = "SIZE", value_parser = parse_size)]
        part_size: u64,
        /// Storage policy of the bucket (controls which EC stripe tests are run)
        #[arg(long, default_value = "ec")]
        policy: commands::test_object::StoragePolicy,
        /// Keep the S3 object after the test (default: delete on success)
        #[arg(long)]
        keep: bool,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },
}

/// Subcommands for `hsc perf`
#[derive(Subcommand)]
enum PerfSubcommand {
    /// Benchmark S3 object operations (PUT / GET / LIST / DELETE)
    Object {
        #[command(subcommand)]
        operation: PerfObjectOp,
    },
}

/// Operation types for `hsc perf object`
#[derive(Subcommand)]
enum PerfObjectOp {
    /// Benchmark PUT (upload) operations
    Put {
        /// S3 URI prefix where objects will be written (s3://bucket[/prefix])
        path: String,
        /// Object size to upload, e.g. 4m, 256k, 1g
        #[arg(long, value_name = "SIZE", value_parser = parse_size)]
        size: u64,
        /// Number of objects to PUT (default: 100; mutually exclusive with --duration)
        #[arg(long, value_name = "N", conflicts_with = "duration")]
        objects: Option<u64>,
        /// Run for a fixed duration, e.g. 30s, 5m (mutually exclusive with --objects)
        #[arg(long, value_name = "DURATION", value_parser = parse_duration, conflicts_with = "objects")]
        duration: Option<u64>,
        /// Number of parallel threads (default: 1)
        #[arg(long, default_value = "1", value_name = "N")]
        threads: usize,
        /// Multipart threshold and part size (default: 8m); uploads exceeding this use multipart
        #[arg(long, default_value = "8m", value_name = "SIZE", value_parser = parse_size,
              conflicts_with = "disable_multipart")]
        part_size: u64,
        /// Always use a single PUT regardless of object size (conflicts with --part-size)
        #[arg(long, conflicts_with = "part_size")]
        disable_multipart: bool,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },
    /// Benchmark GET (download) operations
    Get {
        /// S3 URI prefix where objects to download reside (s3://bucket[/prefix])
        path: String,
        /// Number of GET operations to perform (default: 100; mutually exclusive with --duration)
        #[arg(long, value_name = "N", conflicts_with = "duration")]
        objects: Option<u64>,
        /// Run for a fixed duration, e.g. 30s, 5m (mutually exclusive with --objects)
        #[arg(long, value_name = "DURATION", value_parser = parse_duration, conflicts_with = "objects")]
        duration: Option<u64>,
        /// Number of parallel threads (default: 1)
        #[arg(long, default_value = "1", value_name = "N")]
        threads: usize,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },
    /// Benchmark LIST (ListObjectsV2) operations
    List {
        /// S3 URI prefix to list (s3://bucket[/prefix])
        path: String,
        /// Number of ListObjectsV2 API calls to make (default: 100; mutually exclusive with --duration)
        #[arg(long, value_name = "N", conflicts_with = "duration")]
        objects: Option<u64>,
        /// Run for a fixed duration, e.g. 30s, 5m (mutually exclusive with --objects)
        #[arg(long, value_name = "DURATION", value_parser = parse_duration, conflicts_with = "objects")]
        duration: Option<u64>,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },
    /// Benchmark DELETE operations
    Delete {
        /// S3 URI prefix whose objects will be deleted (s3://bucket[/prefix])
        path: String,
        /// Maximum number of objects to delete (default: 100; mutually exclusive with --duration)
        #[arg(long, value_name = "N", conflicts_with = "duration")]
        objects: Option<u64>,
        /// Run for a fixed duration, e.g. 30s, 5m (mutually exclusive with --objects)
        #[arg(long, value_name = "DURATION", value_parser = parse_duration, conflicts_with = "objects")]
        duration: Option<u64>,
        /// Number of parallel batch-delete threads (default: 1)
        #[arg(long, default_value = "1", value_name = "N")]
        threads: usize,
        /// Emit structured JSON output
        #[arg(long)]
        json: bool,
    },
}

/// Parse a human-readable duration string into seconds.
/// Accepted suffixes: s/S (seconds), m/M (minutes).
/// A bare number is treated as seconds.
fn parse_duration(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix(['M', 'm']) {
        (n, 60u64)
    } else if let Some(n) = s.strip_suffix(['S', 's']) {
        (n, 1u64)
    } else {
        (s, 1u64)
    };
    let base: u64 = num.parse().map_err(|_| {
        format!("invalid duration '{s}': expected a number with optional suffix s/m")
    })?;
    Ok(base * mult)
}

/// Parse a human-readable size string into bytes.
/// Accepted suffixes: b/B (bytes), k/K (KiB), m/M (MiB), g/G (GiB).
/// A bare number is treated as bytes.
fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix(['G', 'g']) {
        (n, 1u64 << 30)
    } else if let Some(n) = s.strip_suffix(['M', 'm']) {
        (n, 1u64 << 20)
    } else if let Some(n) = s.strip_suffix(['K', 'k']) {
        (n, 1u64 << 10)
    } else if let Some(n) = s.strip_suffix(['B', 'b']) {
        (n, 1u64)
    } else {
        (s, 1u64)
    };
    let base: u64 = num
        .parse()
        .map_err(|_| format!("invalid size '{s}': expected a number with optional suffix k/m/g"))?;
    Ok(base * mult)
}

/// Parse an S3 URI into (bucket, prefix) for perf commands.
/// Accepts `s3://bucket` or `s3://bucket/prefix`.
fn parse_s3_path_for_perf(path: &str) -> Result<(String, String), Box<dyn std::error::Error>> {
    let stripped = path
        .strip_prefix("s3://")
        .ok_or_else(|| format!("expected s3:// URI, got '{}'", path))?;
    let (bucket, prefix) = match stripped.find('/') {
        Some(pos) => (stripped[..pos].to_string(), stripped[pos + 1..].to_string()),
        None => (stripped.to_string(), String::new()),
    };
    if bucket.is_empty() {
        return Err("bucket name must not be empty".into());
    }
    Ok((bucket, prefix))
}

/// Resolve effective multipart threshold and chunk size.
/// Priority: CLI flags > config-file defaults (already loaded into config_threshold/chunksize).
fn resolve_multipart(
    disable_multipart: bool,
    part_size: Option<String>,
    config_threshold: u64,
    config_chunksize: u64,
) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    if disable_multipart {
        Ok((u64::MAX, config_chunksize))
    } else if let Some(s) = part_size {
        let size = parse_size(&s)?;
        Ok((size, size))
    } else {
        Ok((config_threshold, config_chunksize))
    }
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
        multipart_threshold: 8388608, // overwritten from ~/.aws/config if present
        multipart_chunksize: 8388608, // overwritten from ~/.aws/config if present
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
    let rdma_provider: Option<std::sync::Arc<dyn rdma::RdmaEndpoint>> =
        match &client_config_clone.rdma_provider {
            Some(p) => match rdma::create_endpoint(p, client_config_clone.debug, rdma::EndpointRole::Client, &Default::default()) {
                Ok(provider) => Some(provider),
                Err(e) => {
                    eprintln!(
                        "Warning: RDMA provider '{}' unavailable, falling back to standard I/O: {}",
                        p, e
                    );
                    None
                }
            },
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
        Commands::Ls {
            path,
            recursive,
            versions,
            human_readable,
            json,
        } => commands::ls::list(&client, path, recursive, versions, human_readable, json).await,
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
            disable_multipart,
            part_size,
        } => {
            let sse_config = commands::cp::SseConfig {
                sse,
                sse_kms_key_id,
                sse_c,
                sse_c_key,
                sse_c_copy_source,
                sse_c_copy_source_key,
            };
            let (threshold, chunksize) = resolve_multipart(
                disable_multipart,
                part_size,
                client_config_clone.multipart_threshold,
                client_config_clone.multipart_chunksize,
            )?;
            commands::cp::copy(
                &client,
                &source,
                &dest,
                recursive,
                include,
                exclude,
                checksum,
                sse_config,
                threshold,
                chunksize,
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
            disable_multipart,
            part_size,
        } => {
            let sse_config = commands::cp::SseConfig {
                sse,
                sse_kms_key_id,
                sse_c,
                sse_c_key,
                sse_c_copy_source,
                sse_c_copy_source_key,
            };
            let (threshold, chunksize) = resolve_multipart(
                disable_multipart,
                part_size,
                client_config_clone.multipart_threshold,
                client_config_clone.multipart_chunksize,
            )?;
            commands::sync::sync(
                &client,
                &source,
                &dest,
                include,
                exclude,
                checksum,
                delete,
                sse_config,
                threshold,
                chunksize,
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
            disable_multipart,
            part_size,
        } => {
            let sse_config = commands::cp::SseConfig {
                sse,
                sse_kms_key_id,
                sse_c,
                sse_c_key,
                sse_c_copy_source,
                sse_c_copy_source_key,
            };
            let (threshold, chunksize) = resolve_multipart(
                disable_multipart,
                part_size,
                client_config_clone.multipart_threshold,
                client_config_clone.multipart_chunksize,
            )?;
            commands::mv::move_files(
                &client,
                &source,
                &dest,
                recursive,
                include,
                exclude,
                sse_config,
                threshold,
                chunksize,
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
            json,
        } => commands::stat::stat(&client, &path, recursive, checksum, json).await,
        Commands::Diff {
            source,
            dest,
            compare_content,
            include,
            exclude,
            json,
        } => {
            commands::diff::diff(
                &client,
                &source,
                &dest,
                compare_content,
                include,
                exclude,
                json,
            )
            .await
        }
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
        Commands::Cmp {
            path1,
            path2,
            algorithm,
            range,
            offset,
            size,
            sse_c,
            sse_c_key,
            json,
        } => {
            commands::cmp::cmp(
                &client,
                &path1,
                &path2,
                &algorithm,
                range,
                offset,
                size,
                sse_c,
                sse_c_key,
                json,
                #[cfg(feature = "rdma")]
                rdma_provider,
            )
            .await
        }
        Commands::Exists { path, json } => commands::exists::exists(&client, &path, json).await,
        Commands::Hash {
            path,
            algorithm,
            json,
        } => commands::hash::hash(&client, &path, &algorithm, json).await,
        Commands::Parts {
            path,
            attributes,
            json,
        } => commands::parts::parts(&client, &path, attributes, json).await,
        Commands::Test {
            subcommand:
                TestSubcommand::Object {
                    bucket,
                    key,
                    file,
                    bytes,
                    chunk_size,
                    part_size,
                    policy,
                    keep,
                    json,
                },
        } => {
            commands::test_object::test_object(
                &client,
                &bucket,
                key.as_deref(),
                file.as_deref(),
                bytes,
                chunk_size,
                part_size,
                &policy,
                keep,
                json,
                #[cfg(feature = "rdma")]
                rdma_provider,
            )
            .await
        }
        Commands::Perf {
            subcommand: PerfSubcommand::Object { operation },
        } => match operation {
            PerfObjectOp::Put {
                path,
                size,
                objects,
                duration,
                threads,
                part_size,
                disable_multipart,
                json,
            } => {
                let (bucket, prefix) = parse_s3_path_for_perf(&path)?;
                commands::perf_object::run_put(
                    &client,
                    &bucket,
                    &prefix,
                    size,
                    objects,
                    duration,
                    threads,
                    part_size,
                    disable_multipart,
                    json,
                )
                .await
            }
            PerfObjectOp::Get {
                path,
                objects,
                duration,
                threads,
                json,
            } => {
                let (bucket, prefix) = parse_s3_path_for_perf(&path)?;
                commands::perf_object::run_get(
                    &client, &bucket, &prefix, objects, duration, threads, json,
                )
                .await
            }
            PerfObjectOp::List {
                path,
                objects,
                duration,
                json,
            } => {
                let (bucket, prefix) = parse_s3_path_for_perf(&path)?;
                commands::perf_object::run_list(&client, &bucket, &prefix, objects, duration, json)
                    .await
            }
            PerfObjectOp::Delete {
                path,
                objects,
                duration,
                threads,
                json,
            } => {
                let (bucket, prefix) = parse_s3_path_for_perf(&path)?;
                commands::perf_object::run_delete(
                    &client, &bucket, &prefix, objects, duration, threads, json,
                )
                .await
            }
        },
    }
}

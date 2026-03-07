use aws_config::BehaviorVersion;
use aws_sdk_s3::Client;
use std::env;

/// Configuration for S3 client creation
#[derive(Clone)]
pub struct S3ClientConfig {
    pub endpoint_url: Option<String>,
    pub region: Option<String>,
    pub profile: Option<String>,
    pub verify_ssl: bool,
    pub debug: bool,
    pub multipart_threshold: u64,
    pub multipart_chunksize: u64,
    /// Enable cuObject RDMA header injection on PUT/GET requests.
    #[allow(dead_code)]
    pub rdma_enabled: bool,
    /// Use the mock RDMA provider instead of cuObject (for testing).
    #[allow(dead_code)]
    pub rdma_mock: bool,
}

impl Default for S3ClientConfig {
    fn default() -> Self {
        Self {
            endpoint_url: None,
            region: None,
            profile: None,
            verify_ssl: true,
            debug: false,
            multipart_threshold: 8388608, // 8MB default
            multipart_chunksize: 8388608, // 8MB default
            rdma_enabled: false,
            rdma_mock: false,
        }
    }
}

/// Initialize and return an S3 client with AWS configuration
///
/// Respects the following environment variables:
/// - AWS_CONFIG_FILE: Path to shared config file
/// - AWS_SHARED_CREDENTIALS_FILE: Path to shared credentials file  
/// - AWS_PROFILE: AWS profile to use
/// - AWS_ENDPOINT_URL: Custom S3 endpoint URL
/// - AWS_REGION: AWS region
/// - AWS_ACCESS_KEY_ID: Access key ID
/// - AWS_SECRET_ACCESS_KEY: Secret access key
/// - AWS_SESSION_TOKEN: Session token (for temporary credentials)
/// - HSC_RDMA: RDMA provider selection.  Accepted values:
///   `cuobject` or `auto` (enable, auto-select provider), `mock` (use mock provider),
///   `true`/`1` (same as `auto`), `false`/`0` (disable).
/// - HSC_RDMA_MOCK: Set to `true` or `1` to use mock RDMA provider
pub async fn create_s3_client(
    mut config: S3ClientConfig,
) -> Result<Client, Box<dyn std::error::Error>> {
    let profile = config
        .profile
        .clone()
        .or_else(|| env::var("AWS_PROFILE").ok())
        .unwrap_or_else(|| "default".to_string());

    // Load multipart settings from config file if not already set
    if config.multipart_threshold == 8388608 && config.multipart_chunksize == 8388608 {
        if let Ok(settings) = load_multipart_settings(&profile) {
            config.multipart_threshold = settings.0;
            config.multipart_chunksize = settings.1;

            if config.debug {
                eprintln!(
                    "Debug: Loaded from config - multipart_threshold: {}, multipart_chunksize: {}",
                    config.multipart_threshold, config.multipart_chunksize
                );
            }
        }
    }

    // RDMA settings are now resolved by the caller (via resolve_rdma_settings) so
    // they are visible before the config is cloned for provider creation.

    // Set up AWS config loader with proper behavior version
    let mut loader = aws_config::defaults(BehaviorVersion::latest());

    // Set profile if specified (from CLI option or environment)
    // (profile was already resolved above for config-file loading)
    if config.debug {
        eprintln!("Debug: Using AWS profile: {}", profile);
    }

    loader = loader.profile_name(&profile);

    // Set region (CLI option > environment > config file > default)
    let region = config.region
        .or_else(|| env::var("AWS_REGION").ok())
        .unwrap_or_else(|| "us-east-1".to_string());
    if config.debug {
        eprintln!("Debug: Using region: {}", region);
    }
    loader = loader.region(aws_sdk_s3::config::Region::new(region));

    // Load the AWS config (respects AWS_CONFIG_FILE and AWS_SHARED_CREDENTIALS_FILE)
    let aws_config = loader.load().await;

    // Build S3-specific config
    let mut s3_config_builder = aws_sdk_s3::config::Builder::from(&aws_config);

    // Set endpoint URL (CLI option > environment)
    let endpoint = config
        .endpoint_url
        .or_else(|| env::var("AWS_ENDPOINT_URL").ok());
    if let Some(endpoint) = endpoint {
        if config.debug {
            eprintln!("Debug: Using custom endpoint: {}", endpoint);
        }
        s3_config_builder = s3_config_builder
            .endpoint_url(&endpoint)
            .force_path_style(true); // Required for S3-compatible services
    }

    // Disable SSL verification if requested.
    // NOTE: The AWS Rust SDK requires a custom HTTP client (e.g. via hyper + rustls)
    // to skip TLS verification.  Until that is wired up, this flag has no effect.
    if !config.verify_ssl {
        eprintln!("Warning: --no-verify-ssl is not yet implemented; SSL verification remains enabled");
    }

    // HTTP request/response tracing is handled by aws_smithy_runtime via the
    // tracing subscriber initialised in main() when --debug is enabled.

    let s3_config = s3_config_builder.build();

    if config.debug {
        eprintln!("Debug: S3 client initialized successfully");
    }

    let client = Client::from_conf(s3_config);
    Ok(client)
}

/// Load multipart settings from AWS config file
/// Returns (threshold, chunksize) from [s3] section
fn load_multipart_settings(profile: &str) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let content = read_aws_config_file()?;
    let mut in_profile_section = false;
    let mut in_s3_section = false;
    let mut threshold: Option<u64> = None;
    let mut chunksize: Option<u64> = None;

    let profile_header = profile_section_header(profile);

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_profile_section = line == profile_header;
            in_s3_section = line == "[s3]";
            continue;
        }
        if in_profile_section || in_s3_section {
            if let Some((key, value)) = line.split_once('=') {
                match key.trim() {
                    "multipart_threshold" => {
                        if let Ok(val) = parse_size_value(value.trim()) {
                            threshold = Some(val);
                        }
                    }
                    "multipart_chunksize" => {
                        if let Ok(val) = parse_size_value(value.trim()) {
                            chunksize = Some(val);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok((threshold.unwrap_or(8388608), chunksize.unwrap_or(8388608)))
}

/// Resolve RDMA settings onto `config` from env vars and the AWS config file.
///
/// Must be called before cloning the config for provider creation.
/// Priority: CLI flag (already on config) > `HSC_RDMA` env > config file > default false.
///
/// The `rdma` key in the config file accepts the same values as `HSC_RDMA`:
/// `auto`, `cuobject`, `mock`, `true`/`1` (enable), `false`/`0` (disable).
pub fn resolve_rdma_settings(config: &mut S3ClientConfig) {
    if config.rdma_enabled || config.rdma_mock {
        return; // CLI flags already set
    }
    let profile = config
        .profile
        .clone()
        .or_else(|| env::var("AWS_PROFILE").ok())
        .unwrap_or_else(|| "default".to_string());
    let cfg_settings = load_rdma_settings(&profile);

    // HSC_RDMA accepts: mock, cuobject, auto, true, 1, false, 0
    let (env_enabled, env_mock) = env::var("HSC_RDMA")
        .map(|v| parse_rdma_provider_value(&v))
        .unwrap_or((false, false));

    config.rdma_enabled = env_enabled || cfg_settings.0;
    config.rdma_mock = env_mock || cfg_settings.1;

    if config.debug && (config.rdma_enabled || config.rdma_mock) {
        eprintln!(
            "Debug: RDMA settings - rdma_enabled: {}, rdma_mock: {}",
            config.rdma_enabled, config.rdma_mock
        );
    }
}

/// Load RDMA settings from the AWS config file for the given profile.
fn load_rdma_settings(profile: &str) -> (bool, bool) {
    let content = match read_aws_config_file() {
        Ok(c) => c,
        Err(_) => return (false, false),
    };

    let mut in_profile_section = false;
    let mut in_s3_section = false;
    let mut result: Option<(bool, bool)> = None;

    let profile_header = profile_section_header(profile);

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_profile_section = line == profile_header;
            in_s3_section = line == "[s3]";
            continue;
        }
        if in_profile_section || in_s3_section {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "rdma" {
                    result = Some(parse_rdma_provider_value(value.trim()));
                }
            }
        }
    }

    result.unwrap_or((false, false))
}

/// Parse size value from config (e.g., "8MB", "10485760", "5M")
fn parse_size_value(value: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let value = value.to_uppercase();

    // Try to parse as plain number first
    if let Ok(num) = value.parse::<u64>() {
        return Ok(num);
    }

    // Parse with suffix (MB, M, KB, K, GB, G)
    let (num_str, multiplier) = if value.ends_with("MB") {
        (&value[..value.len() - 2], 1024 * 1024)
    } else if value.ends_with("M") {
        (&value[..value.len() - 1], 1024 * 1024)
    } else if value.ends_with("KB") {
        (&value[..value.len() - 2], 1024)
    } else if value.ends_with("K") {
        (&value[..value.len() - 1], 1024)
    } else if value.ends_with("GB") {
        (&value[..value.len() - 2], 1024 * 1024 * 1024)
    } else if value.ends_with("G") {
        (&value[..value.len() - 1], 1024 * 1024 * 1024)
    } else {
        return Err("Invalid size format".into());
    };

    let num = num_str.trim().parse::<u64>()?;
    Ok(num * multiplier)
}

/// Read the AWS config file contents.  Returns an error when the file is absent.
fn read_aws_config_file() -> Result<String, Box<dyn std::error::Error>> {
    use std::path::PathBuf;

    let config_path = if let Ok(path) = env::var("AWS_CONFIG_FILE") {
        PathBuf::from(path)
    } else {
        let home = env::var("HOME").or_else(|_| env::var("USERPROFILE"))?;
        PathBuf::from(home).join(".aws").join("config")
    };

    if !config_path.exists() {
        return Err("AWS config file not found".into());
    }

    Ok(std::fs::read_to_string(&config_path)?)
}

/// Return the INI section header string for a given profile name.
fn profile_section_header(profile: &str) -> String {
    if profile == "default" {
        "[default]".to_string()
    } else {
        format!("[profile {}]", profile)
    }
}

/// Parse an RDMA provider value into `(rdma_enabled, use_mock)`.
///
/// | Value                         | rdma_enabled | use_mock |
/// |-------------------------------|:------------:|:--------:|
/// | `mock`                        | true         | true     |
/// | `cuobject`, `auto`, `true`,   | true         | false    |
/// | `1`, `yes`, `on`              |              |          |
/// | `false`, `0`, `no`, `off`, …  | false        | false    |
fn parse_rdma_provider_value(value: &str) -> (bool, bool) {
    match value.to_lowercase().as_str() {
        "mock" => (true, true),
        "cuobject" | "auto" | "true" | "1" | "yes" | "on" => (true, false),
        _ => (false, false),
    }
}

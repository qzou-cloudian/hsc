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
pub async fn create_s3_client(
    mut config: S3ClientConfig,
) -> Result<Client, Box<dyn std::error::Error>> {
    // Load multipart settings from config file if not already set
    if config.multipart_threshold == 8388608 && config.multipart_chunksize == 8388608 {
        let profile = config
            .profile
            .clone()
            .or_else(|| env::var("AWS_PROFILE").ok())
            .unwrap_or_else(|| "default".to_string());

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

    // Set up AWS config loader with proper behavior version
    let mut loader = aws_config::defaults(BehaviorVersion::latest());

    // Set profile if specified (from CLI option or environment)
    let profile = config
        .profile
        .or_else(|| env::var("AWS_PROFILE").ok())
        .unwrap_or_else(|| "default".to_string());

    if config.debug {
        eprintln!("Debug: Using AWS profile: {}", profile);
    }

    loader = loader.profile_name(&profile);

    // Set region (CLI option > environment > config file)
    if let Some(region) = config.region.or_else(|| env::var("AWS_REGION").ok()) {
        if config.debug {
            eprintln!("Debug: Using region: {}", region);
        }
        loader = loader.region(aws_sdk_s3::config::Region::new(region));
    }

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

    // Disable SSL verification if requested
    // Note: This requires additional setup in production use
    if !config.verify_ssl {
        if config.debug {
            eprintln!("Debug: SSL verification disabled");
        }
        // SSL verification is controlled at the HTTP client level
        // For now, we log the setting. Full implementation would require
        // custom HTTP client configuration.
        eprintln!("Warning: --no-verify-ssl is noted but requires custom HTTP client setup");
    }

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
    use std::fs;
    use std::path::PathBuf;

    // Determine config file path
    let config_path = if let Ok(path) = env::var("AWS_CONFIG_FILE") {
        PathBuf::from(path)
    } else {
        let home = env::var("HOME").or_else(|_| env::var("USERPROFILE"))?;
        PathBuf::from(home).join(".aws").join("config")
    };

    if !config_path.exists() {
        return Err("AWS config file not found".into());
    }

    let content = fs::read_to_string(&config_path)?;

    // Parse config file looking for [profile <name>] or [s3] section
    let mut in_profile_section = false;
    let mut in_s3_section = false;
    let mut threshold: Option<u64> = None;
    let mut chunksize: Option<u64> = None;

    let profile_header = if profile == "default" {
        "[default]".to_string()
    } else {
        format!("[profile {}]", profile)
    };

    for line in content.lines() {
        let line = line.trim();

        // Check for section headers
        if line.starts_with('[') && line.ends_with(']') {
            in_profile_section = line == profile_header;
            in_s3_section = line == "[s3]";
            continue;
        }

        // Parse settings in relevant sections
        if in_profile_section || in_s3_section {
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();

                match key {
                    "multipart_threshold" => {
                        if let Ok(val) = parse_size_value(value) {
                            threshold = Some(val);
                        }
                    }
                    "multipart_chunksize" => {
                        if let Ok(val) = parse_size_value(value) {
                            chunksize = Some(val);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Return values from [s3] section, or profile-specific if available
    Ok((threshold.unwrap_or(8388608), chunksize.unwrap_or(8388608)))
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

use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::context::BeforeTransmitInterceptorContextMut;
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::config_bag::ConfigBag;
use aws_smithy_types::timeout::TimeoutConfig;
use std::env;
use std::sync::Arc;
use std::time::Duration;

use crate::debug_interceptor::DebugInterceptor;
use crate::redirect_interceptor::{RedirectInterceptor, RedirectRetryClassifier};

/// A `rustls` `ServerCertVerifier` that accepts any certificate.
/// Used only when `--no-verify-ssl` is requested (e.g. self-signed cert testing).
#[derive(Debug)]
struct AcceptAnyServerCert;

impl rustls::client::ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

/// Interceptor that removes AWS auth headers so requests are sent unsigned.
/// Used when `--no-sign-request` is set (e.g. accessing public buckets).
#[derive(Debug)]
struct NoSignRequestInterceptor;

impl Intercept for NoSignRequestInterceptor {
    fn name(&self) -> &'static str {
        "NoSignRequestInterceptor"
    }

    fn modify_before_transmit(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let req = context.request_mut();
        req.headers_mut().remove("authorization");
        req.headers_mut().remove("x-amz-security-token");
        Ok(())
    }
}

/// Interceptor that injects user-supplied headers into every outgoing request.
/// Used when `--custom-header KEY:VALUE` is specified.
///
/// Several header families are only valid on object-creation requests
/// (`PutObject`, `CreateMultipartUpload`) and are intentionally skipped for
/// `UploadPart` and `CompleteMultipartUpload` (detected by `partNumber=` /
/// `uploadId=` query params).  Sending them on part requests results in S3
/// errors such as "Metadata cannot be specified in this context."  The
/// filtered families are:
///
/// * `x-amz-meta-*`   — user-defined object metadata
/// * `x-amz-acl`      — canned ACL
/// * `x-amz-grant-*`  — explicit ACL grants
/// * `x-amz-tagging`  — object tags
#[derive(Debug)]
struct CustomHeadersInterceptor {
    headers: Vec<(String, String)>,
}

/// Returns true when the header should be skipped for `UploadPart` /
/// `CompleteMultipartUpload` requests.
fn is_object_creation_only_header(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("x-amz-meta-")
        || lower == "x-amz-acl"
        || lower.starts_with("x-amz-grant-")
        || lower == "x-amz-tagging"
}

impl Intercept for CustomHeadersInterceptor {
    fn name(&self) -> &'static str {
        "CustomHeadersInterceptor"
    }

    fn modify_before_transmit(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let req = context.request_mut();
        let has_upload_params = req
            .uri()
            .split_once('?')
            .map(|(_, q)| {
                q.split('&')
                    .any(|param| param.starts_with("partNumber=") || param.starts_with("uploadId="))
            })
            .unwrap_or(false);
        for (key, value) in &self.headers {
            if has_upload_params && is_object_creation_only_header(key) {
                continue;
            }
            req.headers_mut().append(key.clone(), value.clone());
        }
        Ok(())
    }
}

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
    /// Read timeout in seconds; `None` or `0` means no timeout.
    pub read_timeout_secs: Option<u64>,
    /// Connect timeout in seconds; `None` or `0` means no timeout.
    pub connect_timeout_secs: Option<u64>,
    /// Extra headers to inject into every request (parsed KEY:VALUE pairs).
    pub custom_headers: Vec<String>,
    /// When `true`, auth headers are stripped so requests are sent unsigned.
    pub no_sign_request: bool,
    /// Normalized RDMA provider name (`"mock"`, `"cuobj"`), or `None` to disable RDMA.
    #[allow(dead_code)]
    pub rdma_provider: Option<String>,
}

impl Default for S3ClientConfig {
    fn default() -> Self {
        Self {
            endpoint_url: None,
            region: None,
            profile: None,
            verify_ssl: true,
            debug: false,
            multipart_threshold: 8388608, // 8 MiB default
            multipart_chunksize: 8388608, // 8 MiB default
            read_timeout_secs: None,
            connect_timeout_secs: None,
            custom_headers: Vec::new(),
            no_sign_request: false,
            rdma_provider: None,
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
///   `cuobj` or `auto` (enable, auto-select provider), `mock` (use mock provider),
///   `true`/`1` (same as `auto`), `false`/`0` (disable).
/// - HSC_RDMA_MOCK: Set to `true` or `1` to use mock RDMA provider
/// - HSC_DEBUG: Set to a non-empty value to enable request/response header logging
pub async fn create_s3_client(
    mut config: S3ClientConfig,
) -> Result<Client, Box<dyn std::error::Error>> {
    let profile = config
        .profile
        .clone()
        .or_else(|| env::var("AWS_PROFILE").ok())
        .unwrap_or_else(|| "default".to_string());

    // Load multipart settings from config file if not already set by CLI flags.
    // Priority: CLI flags > ~/.aws/config [s3] section > 8 MiB default.
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

    // RDMA settings are now resolved by the caller
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
    let region = config
        .region
        .or_else(|| env::var("AWS_REGION").ok())
        .unwrap_or_else(|| "us-east-1".to_string());
    if config.debug {
        eprintln!("Debug: Using region: {}", region);
    }
    loader = loader.region(aws_sdk_s3::config::Region::new(region));

    // Apply connect/read timeout configuration
    {
        let mut tb = TimeoutConfig::builder();
        if let Some(t) = config.connect_timeout_secs {
            if t > 0 {
                tb = tb.connect_timeout(Duration::from_secs(t));
            }
        }
        if let Some(t) = config.read_timeout_secs {
            if t > 0 {
                tb = tb.read_timeout(Duration::from_secs(t));
            }
        }
        loader = loader.timeout_config(tb.build());
    }

    // Provide empty credentials when --no-sign-request is set so the SDK does
    // not fail trying to resolve real credentials before the interceptor strips
    // the auth headers.
    if config.no_sign_request {
        loader = loader.credentials_provider(Credentials::new(
            "",
            "",
            None::<String>,
            None,
            "anonymous",
        ));
    }

    // Load the AWS config (respects AWS_CONFIG_FILE and AWS_SHARED_CREDENTIALS_FILE)
    let aws_config = loader.load().await;

    // Build S3-specific config
    let mut s3_config_builder = aws_sdk_s3::config::Builder::from(&aws_config);

    // Do not inject checksums automatically (BehaviorVersion::latest() defaults to
    // WhenSupported, which adds CRC32C to CreateMultipartUpload even when the caller
    // did not request a checksum).  S3-compatible servers (e.g. Cloudian) then require
    // per-part checksums in CompleteMultipartUpload, causing a 400 InvalidRequest.
    // Set WhenRequired so the SDK only adds checksums when hsc explicitly requests them.
    s3_config_builder = s3_config_builder
        .request_checksum_calculation(aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired);

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

    // Disable SSL verification if requested (e.g. self-signed cert testing).
    if !config.verify_ssl {
        let tls_cfg = std::sync::Arc::new(
            rustls::ClientConfig::builder()
                .with_safe_defaults()
                .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCert))
                .with_no_client_auth(),
        );
        let https: hyper_rustls::HttpsConnector<hyper::client::HttpConnector> =
            hyper_rustls::HttpsConnectorBuilder::new()
                .with_tls_config((*tls_cfg).clone())
                .https_or_http()
                .enable_http1()
                .enable_http2()
                .build();
        let smithy_client =
            aws_smithy_runtime::client::http::hyper_014::HyperClientBuilder::new().build(https);
        s3_config_builder = s3_config_builder.http_client(smithy_client);
    }

    // Attach DebugInterceptor when debug mode is enabled.
    if config.debug {
        s3_config_builder = s3_config_builder.interceptor(DebugInterceptor::default());
    }

    // Add custom headers interceptor if any --custom-header flags were given.
    if !config.custom_headers.is_empty() {
        let mut parsed: Vec<(String, String)> = Vec::new();
        for h in &config.custom_headers {
            let parts: Vec<&str> = h.splitn(2, ':').collect();
            if parts.len() == 2 {
                parsed.push((parts[0].trim().to_string(), parts[1].trim().to_string()));
            } else {
                eprintln!(
                    "Warning: ignoring malformed --custom-header (expected KEY:VALUE): {}",
                    h
                );
            }
        }
        if !parsed.is_empty() {
            s3_config_builder =
                s3_config_builder.interceptor(CustomHeadersInterceptor { headers: parsed });
        }
    }

    // Strip auth headers when --no-sign-request is set.
    if config.no_sign_request {
        s3_config_builder = s3_config_builder.interceptor(NoSignRequestInterceptor);
    }

    // Enable automatic HTTP 307 Temporary Redirect following.
    // S3-compatible services (e.g. Cloudian HyperStore) use 307 for load-balancing:
    // a node redirects the client to the node that owns the data.  The interceptor
    // stores the redirect Location in the config bag; the classifier triggers a retry;
    // and modify_before_signing updates the URI so the retry is correctly re-signed.
    s3_config_builder = s3_config_builder
        .interceptor(RedirectInterceptor::default())
        .retry_classifier(RedirectRetryClassifier::default());

    let s3_config = s3_config_builder.build();

    if config.debug {
        eprintln!("Debug: S3 client initialized successfully");
    }

    let client = Client::from_conf(s3_config);
    Ok(client)
}

/// Resolve RDMA settings onto `config` from env vars and the AWS config file.
///
/// Must be called before cloning the config for provider creation.
/// Priority: CLI flag (already on config) > `HSC_RDMA` env > config file > default false.
///
/// The `rdma` key in the config file accepts the same values as `HSC_RDMA`:
/// `auto`, `cuobj`, `mock`, `true`/`1` (enable), `false`/`0` (disable).
pub fn resolve_rdma_settings(config: &mut S3ClientConfig) {
    if config.rdma_provider.is_some() {
        // Normalize the CLI-provided value (e.g. "auto" → "cuobj").
        config.rdma_provider = config
            .rdma_provider
            .as_deref()
            .and_then(parse_rdma_provider_value);
        return;
    }
    let profile = config
        .profile
        .clone()
        .or_else(|| env::var("AWS_PROFILE").ok())
        .unwrap_or_else(|| "default".to_string());
    let cfg_setting = load_rdma_settings(&profile);

    // HSC_RDMA accepts: mock, cuobj, auto, true, 1, false, 0
    let env_provider = env::var("HSC_RDMA")
        .ok()
        .and_then(|v| parse_rdma_provider_value(&v));

    // env takes priority over config file
    config.rdma_provider = env_provider.or(cfg_setting);

    if config.debug && config.rdma_provider.is_some() {
        eprintln!(
            "Debug: RDMA settings - provider: {:?}",
            config.rdma_provider
        );
    }
}

/// Load RDMA settings from the AWS config file for the given profile.
fn load_rdma_settings(profile: &str) -> Option<String> {
    let content = match read_aws_config_file() {
        Ok(c) => c,
        Err(_) => return None,
    };

    let mut in_profile_section = false;
    let mut in_s3_section = false;
    let mut result: Option<String> = None;

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
                    result = parse_rdma_provider_value(value.trim());
                }
            }
        }
    }

    result
}

/// Load multipart settings from AWS config file.
/// Returns (threshold, chunksize) from [s3] or profile section.
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

/// Parse a size value from config (e.g., "8MB", "10485760", "5M").
fn parse_size_value(value: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let value = value.to_uppercase();
    if let Ok(num) = value.parse::<u64>() {
        return Ok(num);
    }
    let (num_str, multiplier) = if value.ends_with("GB") {
        (&value[..value.len() - 2], 1024 * 1024 * 1024u64)
    } else if value.ends_with('G') {
        (&value[..value.len() - 1], 1024 * 1024 * 1024u64)
    } else if value.ends_with("MB") {
        (&value[..value.len() - 2], 1024 * 1024u64)
    } else if value.ends_with('M') {
        (&value[..value.len() - 1], 1024 * 1024u64)
    } else if value.ends_with("KB") {
        (&value[..value.len() - 2], 1024u64)
    } else if value.ends_with('K') {
        (&value[..value.len() - 1], 1024u64)
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

/// Parse an RDMA provider value into a normalized provider name.
///
/// | Value                         | Result          |
/// |-------------------------------|-----------------|
/// | `mock`                        | `Some("mock")`  |
/// | `cuobj`, `auto`, `true`,      | `Some("cuobj")` |
/// | `1`, `yes`, `on`              |                 |
/// | `false`, `0`, `no`, `off`, …  | `None`          |
fn parse_rdma_provider_value(value: &str) -> Option<String> {
    match value.to_lowercase().as_str() {
        "mock" => Some("mock".to_owned()),
        "cuobj" | "auto" | "true" | "1" | "yes" | "on" => Some("cuobj".to_owned()),
        _ => None,
    }
}

//! Smithy `Intercept` implementation that injects RDMA token headers into S3
//! HTTP requests and processes RDMA reply headers from S3 responses.
//!
//! # Usage
//!
//! Build an interceptor once per request with the pre-generated RDMA token,
//! then attach it via the SDK's `customize().interceptor(…)` API:
//!
//! ```rust,ignore
//! use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
//! use crate::{MockRdmaProvider, RdmaClientProvider, RdmaClientChannel, RdmaInterceptor};
//!
//! let provider: Arc<dyn RdmaClientProvider> = Arc::new(MockRdmaProvider::new(false, "/dev/shm".into(), 0));
//! let confirmed = Arc::new(AtomicBool::new(false));
//! // Bind buffer to create a channel, then generate a token from it.
//! let channel: Arc<dyn RdmaClientChannel> = Arc::from(provider.bind(buf.as_mut_ptr(), size, key.as_bytes())?);
//! let token = channel.prepare_get_token(0, size)?;
//! let interceptor = RdmaInterceptor::new_get(Arc::clone(&channel), token, size, Arc::clone(&confirmed), debug);
//!
//! let resp = client.get_object()...customize().interceptor(interceptor).send().await?;
//! if confirmed.load(Ordering::Acquire) { /* use RDMA buffer */ }
//! ```

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::context::{
    BeforeDeserializationInterceptorContextMut, BeforeDeserializationInterceptorContextRef,
    BeforeTransmitInterceptorContextMut,
};
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::config_bag::ConfigBag;

use s3_rdma::RdmaClientChannel;
use s3_rdma::provider::{RDMA_BYTES_HEADER, RDMA_REPLY_HEADER, RDMA_TOKEN_HEADER};

// ── RdmaInterceptor ──────────────────────────────────────────────────────────

/// Checksum headers the server may send; we strip them on RDMA replies so the
/// SDK doesn't validate the (empty) HTTP body against the real-data checksum.
const CHECKSUM_HEADERS: &[&str] = &[
    "x-amz-checksum-crc32",
    "x-amz-checksum-crc32c",
    "x-amz-checksum-sha1",
    "x-amz-checksum-sha256",
    "x-amz-checksum-crc64nvme",
];

/// Smithy interceptor that:
/// 1. Injects `x-amz-rdma-token` and `x-amz-rdma-size` request headers.
/// 2. For PUT requests: injects `x-amz-sdk-checksum-algorithm` and
///    `x-amz-checksum-<alg>` headers with a precomputed checksum of the data.
/// 3. On response: if `x-amz-rdma-reply` is present, strips checksum headers
///    so the SDK does not validate the empty HTTP body, and sets
///    `rdma_confirmed` so the caller knows to use the pre-filled RDMA buffer.
pub struct RdmaInterceptor {
    channel: Arc<dyn RdmaClientChannel>,
    /// Pre-generated RDMA descriptor token (may be `|`-separated for multi-token).
    token: Vec<u8>,
    /// Byte count for the transfer (fallback for `x-amz-rdma-bytes` when absent).
    size: usize,
    /// Set to `true` by the interceptor when the server confirms RDMA.
    rdma_confirmed: Arc<AtomicBool>,
    debug: bool,
    /// Checksum algorithm name for PUT requests (e.g. "CRC32C"), if requested.
    checksum_algorithm: Option<String>,
    /// Base64-encoded precomputed checksum of the data, if requested.
    checksum_value: Option<String>,
}

impl std::fmt::Debug for RdmaInterceptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RdmaInterceptor")
            .field("token_len", &self.token.len())
            .field("size", &self.size)
            .field("debug", &self.debug)
            .finish()
    }
}

impl RdmaInterceptor {
    /// Create an interceptor for a PUT (upload) request.
    ///
    /// `checksum_algorithm` and `checksum_value` are injected as
    /// `x-amz-sdk-checksum-algorithm` and `x-amz-checksum-<alg>` headers.
    /// Both must be `Some` or both `None`; passing one without the other is
    /// silently ignored.
    pub fn new_put(
        channel: Arc<dyn RdmaClientChannel>,
        token: Vec<u8>,
        size: usize,
        rdma_confirmed: Arc<AtomicBool>,
        debug: bool,
        checksum_algorithm: Option<String>,
        checksum_value: Option<String>,
    ) -> Self {
        Self {
            channel,
            token,
            size,
            rdma_confirmed,
            debug,
            checksum_algorithm,
            checksum_value,
        }
    }

    /// Create an interceptor for a GET (download) request.
    pub fn new_get(
        channel: Arc<dyn RdmaClientChannel>,
        token: Vec<u8>,
        size: usize,
        rdma_confirmed: Arc<AtomicBool>,
        debug: bool,
    ) -> Self {
        Self {
            channel,
            token,
            size,
            rdma_confirmed,
            debug,
            checksum_algorithm: None,
            checksum_value: None,
        }
    }
}

impl Intercept for RdmaInterceptor {
    fn name(&self) -> &'static str {
        "RdmaInterceptor"
    }

    /// Before signing: strip any SDK-auto-computed checksum headers and,
    /// if a precomputed checksum was supplied, inject it so the correct headers
    /// are included in the SigV4 signature.  Must happen here — changes in
    /// `modify_before_transmit` are after signing and would produce a
    /// `SignatureDoesNotMatch` error.
    fn modify_before_signing(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let headers = context.request_mut().headers_mut();
        // Always remove SDK-auto-computed checksum headers for all algorithms.
        // For RDMA the HTTP body is empty, so any SDK-computed checksum would be
        // wrong (CRC32 of "" rather than CRC32 of the actual data).
        for h in &[
            "x-amz-checksum-crc32",
            "x-amz-checksum-crc32c",
            "x-amz-checksum-sha1",
            "x-amz-checksum-sha256",
            "x-amz-sdk-checksum-algorithm",
        ] {
            headers.remove(*h);
        }
        // Inject our precomputed checksum only when one was requested.
        if let (Some(alg), Some(val)) = (&self.checksum_algorithm, &self.checksum_value) {
            if self.debug {
                eprintln!("[RdmaInterceptor] injecting checksum headers before signing: alg={alg}");
            }
            headers.insert("x-amz-sdk-checksum-algorithm", alg.clone());
            headers.insert(
                format!("x-amz-checksum-{}", alg.to_lowercase()),
                val.clone(),
            );
        }
        Ok(())
    }

    /// Inject `x-amz-rdma-token` request header, plus checksum headers when a
    /// precomputed checksum was supplied (PUT requests only).
    fn modify_before_transmit(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let token_value = String::from_utf8_lossy(&self.token).into_owned();

        if self.debug {
            eprintln!(
                "[RdmaInterceptor] injecting '{}' ({} bytes): '{token_value}'",
                RDMA_TOKEN_HEADER,
                self.token.len()
            );
        }

        let headers = context.request_mut().headers_mut();
        headers.insert(RDMA_TOKEN_HEADER, token_value);

        Ok(())
    }

    /// After network: process the RDMA reply token if present.
    fn read_after_transmit(
        &self,
        context: &BeforeDeserializationInterceptorContextRef<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        if let Some(reply_header_value) = context.response().headers().get(RDMA_REPLY_HEADER) {
            if self.debug {
                eprintln!(
                    "[RdmaInterceptor] RDMA reply received: '{}'",
                    reply_header_value
                );
            }
            self.rdma_confirmed.store(true, Ordering::Release);

            // Parse the reply header as a numeric HTTP status code (200/204/206 = success, 501 = fallback).
            let rdma_status: u16 = reply_header_value.trim().parse().unwrap_or(0);

            // Read the byte-count header (x-amz-rdma-bytes) if present.
            let transferred: usize = context
                .response()
                .headers()
                .get(RDMA_BYTES_HEADER)
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(self.size);

            let channel = Arc::clone(&self.channel);
            let request_token = self.token.clone();
            let debug = self.debug;
            channel
                .process_rdma_reply(
                    rdma_status,
                    &request_token,
                    transferred,
                    Box::new(move |result| {
                        if result.error_code != 0 {
                            eprintln!("[RdmaInterceptor] RDMA error (code={})", result.error_code);
                        } else if debug {
                            eprintln!("[RdmaInterceptor] RDMA transfer completed");
                        }
                    }),
                )
                .map_err(|e| Box::new(e) as BoxError)?;
        }
        Ok(())
    }

    /// Before deserialization: if RDMA was confirmed, strip checksum headers so
    /// the SDK does not validate them against the empty HTTP body.
    fn modify_before_deserialization(
        &self,
        context: &mut BeforeDeserializationInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        if self.rdma_confirmed.load(Ordering::Acquire) {
            let headers = context.response_mut().headers_mut();
            for name in CHECKSUM_HEADERS {
                headers.remove(*name);
            }
            if self.debug {
                eprintln!("[RdmaInterceptor] stripped checksum headers for RDMA response");
            }
        }
        Ok(())
    }
}

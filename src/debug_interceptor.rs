//! Smithy `Intercept` implementation that prints outgoing request headers and
//! incoming response headers to stderr.
//!
//! Enabled when `--debug` is passed or the `HSC_DEBUG` environment variable is
//! set to a non-empty value.
//!
//! ## Trailer headers
//!
//! When the AWS SDK uses `aws-chunked` encoding (e.g. for checksum uploads), the
//! checksum value is sent as an HTTP trailer appended after the body, not as a
//! regular request header.  The trailer is computed during body streaming, so its
//! value is not yet known in `read_before_transmit`.  S3 echoes each trailer
//! back in the response headers, so `read_after_transmit` reads the announced
//! trailer names (stored from the preceding `read_before_transmit` call) and
//! prints their values — sourced from the S3 response echo — as outgoing `>T`
//! lines immediately before the response.

use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::context::{
    BeforeDeserializationInterceptorContextRef, BeforeTransmitInterceptorContextRef,
};
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::config_bag::ConfigBag;
use std::sync::Mutex;

/// Logs outgoing request headers, HTTP trailers, and incoming response headers to stderr.
#[derive(Debug, Default)]
pub struct DebugInterceptor {
    /// Header names announced via `x-amz-trailer` in the most recent request.
    /// Populated in `read_before_transmit`; consumed in `read_after_transmit`.
    announced_trailers: Mutex<Vec<String>>,
}

impl Intercept for DebugInterceptor {
    fn name(&self) -> &'static str {
        "DebugInterceptor"
    }

    /// Print outgoing request method, URI, and headers.
    ///
    /// Uses `read_before_transmit` (read-only, runs after all
    /// `modify_before_transmit` hooks) so that headers injected by other
    /// interceptors — e.g. `RdmaInterceptor` — are visible in the output.
    ///
    /// Also records any `x-amz-trailer` header values so that the actual
    /// trailer values can be printed in `read_after_transmit` once the S3
    /// response echoes them back.
    fn read_before_transmit(
        &self,
        context: &BeforeTransmitInterceptorContextRef<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let req = context.request();
        eprintln!("[hsc] > {} {}", req.method(), req.uri());

        let mut trailers = self.announced_trailers.lock().unwrap();
        trailers.clear();

        for (name, value) in req.headers().iter() {
            eprintln!("[hsc] > {}: {}", name, value);
            if name.eq_ignore_ascii_case("x-amz-trailer") {
                trailers.push(value.to_string());
            }
        }
        Ok(())
    }

    /// Print incoming response status and headers.
    ///
    /// Before printing the response, prints the actual values of any HTTP
    /// trailers that were announced in the request (via `x-amz-trailer`).
    /// S3 echoes trailer values back as response headers, so the values are
    /// read from the response and printed with a `>T` direction marker to
    /// distinguish them from regular request headers and response headers.
    fn read_after_transmit(
        &self,
        context: &BeforeDeserializationInterceptorContextRef<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let resp = context.response();

        // Print request trailers (echoed by S3 in the response) before the response.
        {
            let trailers = self.announced_trailers.lock().unwrap();
            for trailer_name in trailers.iter() {
                if let Some(value) = resp.headers().get(trailer_name) {
                    eprintln!("[hsc] >T {}: {}", trailer_name, value);
                }
            }
        }

        eprintln!("[hsc] < {}", resp.status());
        for (name, value) in resp.headers().iter() {
            eprintln!("[hsc] < {}: {}", name, value);
        }
        Ok(())
    }
}

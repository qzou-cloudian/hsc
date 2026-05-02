//! Smithy interceptor and retry classifier for HTTP 307 Temporary Redirect.
//!
//! S3-compatible services (e.g. Cloudian HyperStore) use 307 Temporary Redirect
//! for load-balancing: the node that receives a request may redirect the client to
//! the node that actually owns the data.
//!
//! The AWS SDK for Rust does not automatically follow 307 redirects, so without
//! this module such requests fail with a `TemporaryRedirect` error.
//!
//! ## How it works
//!
//! 1. **`RedirectRetryClassifier`** – classifies any 307 HTTP response as a transient
//!    error so the SDK's standard retry loop triggers a new attempt.
//!
//! 2. **`RedirectInterceptor`** – hooks into two SDK interceptor points:
//!    * `modify_before_deserialization`: when a 307 response is received, the
//!      `Location` header value is stored in the config bag under `RedirectTarget`.
//!    * `modify_before_signing`: if a `RedirectTarget` is present in the config bag,
//!      the request URI's scheme and authority are updated to match the redirect target
//!      *before* SigV4 signing occurs.  This ensures the signature is computed for
//!      the redirected host and the retry is correctly authenticated.
//!
//! The config bag persists across retry attempts, so the stored `RedirectTarget` is
//! available on the very next attempt.  Up to `MAX_REDIRECTS` consecutive 307
//! responses are followed; after that the response is passed through as-is.

use std::sync::{Arc, Mutex};

use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::context::{
    BeforeDeserializationInterceptorContextMut, BeforeTransmitInterceptorContextMut,
    InterceptorContext,
};
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_runtime_api::client::retries::classifiers::{
    ClassifyRetry, RetryAction, RetryClassifierPriority,
};
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::config_bag::{ConfigBag, Storable, StoreReplace};

/// Maximum number of consecutive 307 redirects to follow per operation.
const MAX_REDIRECTS: u32 = 5;

/// Stored redirect target: the scheme+authority extracted from the `Location` header
/// of a 307 response.  Survives across retry attempts in the config bag.
#[derive(Clone, Debug)]
struct RedirectTarget(String);

impl Storable for RedirectTarget {
    type Storer = StoreReplace<RedirectTarget>;
}

// ---------------------------------------------------------------------------
// RedirectInterceptor
// ---------------------------------------------------------------------------

/// Smithy interceptor that enables following HTTP 307 Temporary Redirect responses.
///
/// Add this interceptor to the S3 config builder together with
/// [`RedirectRetryClassifier`] to activate automatic 307 redirect following.
#[derive(Debug, Default)]
pub struct RedirectInterceptor {
    /// Counts how many 307 redirects have been followed for the current operation.
    redirect_count: Arc<Mutex<u32>>,
}

impl Intercept for RedirectInterceptor {
    fn name(&self) -> &'static str {
        "RedirectInterceptor"
    }

    /// Before signing each attempt: if a redirect target was stored in the config bag
    /// by a previous attempt, update the request URI's scheme and authority to point
    /// at the redirect target so that SigV4 signs the request for the new host.
    fn modify_before_signing(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        if let Some(redirect) = cfg.load::<RedirectTarget>() {
            let endpoint = redirect.0.clone();
            context
                .request_mut()
                .uri_mut()
                .set_endpoint(&endpoint)
                .map_err(|e| format!("307 redirect: failed to update request URI: {e}"))?;
        }
        Ok(())
    }

    /// After receiving the HTTP response but before deserialization: if the response
    /// is a 307, extract the `Location` header and persist it so the next retry
    /// attempt is directed to the redirect target.
    fn modify_before_deserialization(
        &self,
        context: &mut BeforeDeserializationInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let response = context.response();
        if response.status().as_u16() != 307 {
            // Not a redirect – reset the counter so subsequent operations start fresh.
            *self.redirect_count.lock().unwrap() = 0;
            return Ok(());
        }

        let mut count = self.redirect_count.lock().unwrap();
        if *count >= MAX_REDIRECTS {
            // Too many redirects; let the SDK surface the 307 as an error.
            return Ok(());
        }
        *count += 1;

        if let Some(location) = response.headers().get("location") {
            let endpoint = extract_scheme_and_authority(location);
            if let Some(endpoint) = endpoint {
                cfg.interceptor_state().store_put(RedirectTarget(endpoint));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// RedirectRetryClassifier
// ---------------------------------------------------------------------------

/// Retry classifier that treats HTTP 307 responses as transient errors, causing
/// the SDK's standard retry loop to issue a new attempt.
///
/// Use this alongside [`RedirectInterceptor`]; the interceptor stores the redirect
/// target so the retry is sent to the correct host.
#[derive(Debug, Default)]
pub struct RedirectRetryClassifier;

impl ClassifyRetry for RedirectRetryClassifier {
    fn classify_retry(&self, ctx: &InterceptorContext) -> RetryAction {
        if let Some(response) = ctx.response() {
            if response.status().as_u16() == 307 {
                return RetryAction::transient_error();
            }
        }
        RetryAction::NoActionIndicated
    }

    fn name(&self) -> &'static str {
        "RedirectRetryClassifier"
    }

    /// Run with lower priority than the modeled-as-retryable classifier so that
    /// service-modeled retry decisions take precedence.
    fn priority(&self) -> RetryClassifierPriority {
        RetryClassifierPriority::run_before(
            RetryClassifierPriority::modeled_as_retryable_classifier(),
        )
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the scheme and authority (`scheme://host[:port]`) from a URL string.
///
/// Returns `None` if the string does not contain `://` or has no authority component.
///
/// Example:
/// ```text
/// "http://s3-node2.example.com:8080/bucket/key?x=1"  →  Some("http://s3-node2.example.com:8080")
/// ```
fn extract_scheme_and_authority(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = match rest.find('/') {
        Some(pos) => &rest[..pos],
        None => rest,
    };
    if authority.is_empty() {
        return None;
    }
    Some(format!("{}://{}", scheme, authority))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_scheme_and_authority() {
        assert_eq!(
            extract_scheme_and_authority("http://s3-node2.example.com:8080/bucket/key?x=1"),
            Some("http://s3-node2.example.com:8080".to_string())
        );
        assert_eq!(
            extract_scheme_and_authority("https://s3.amazonaws.com/bucket/key"),
            Some("https://s3.amazonaws.com".to_string())
        );
        assert_eq!(
            extract_scheme_and_authority("http://192.168.1.5:8080"),
            Some("http://192.168.1.5:8080".to_string())
        );
        assert_eq!(
            extract_scheme_and_authority("not-a-url"),
            None
        );
        assert_eq!(
            extract_scheme_and_authority("http:///no-authority"),
            None
        );
    }
}

//! Mock RDMA provider – no NVIDIA hardware required.
//!
//! Suitable for testing and development.  Memory suitability is simulated
//! (buffers ≥ 1 MiB are accepted).  Tokens are deterministic ASCII strings.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::RdmaError;
use crate::provider::{RdmaCompletionCallback, RdmaCompletionResult, RdmaProvider};

fn env_or(var: &str, default: &'static str) -> &'static str {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => Box::leak(v.into_boxed_str()),
        _ => default,
    }
}

/// Pure-Rust mock RDMA provider.  Does not require the NVIDIA cuObject SDK.
pub struct MockRdmaProvider {
    debug: bool,
    registered: Mutex<HashMap<usize, usize>>,
    token_header: &'static str,
    reply_header: &'static str,
}

impl std::fmt::Debug for MockRdmaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockRdmaProvider")
            .field("debug", &self.debug)
            .finish()
    }
}

impl MockRdmaProvider {
    /// Create a new mock provider.  Set `debug = true` for trace output.
    pub fn new(debug: bool) -> Self {
        Self {
            debug,
            registered: Mutex::new(HashMap::new()),
            token_header: env_or("CUOBJECT_RDMA_TOKEN_HEADER_NAME", "x-amz-rdma-token"),
            reply_header: env_or("CUOBJECT_RDMA_REPLY_HEADER_NAME", "x-amz-rdma-reply"),
        }
    }

    fn dbg(&self, msg: &str) {
        if self.debug {
            eprintln!("[MockRdmaProvider] {msg}");
        }
    }
}

impl RdmaProvider for MockRdmaProvider {
    fn name(&self) -> &str {
        "MockRdmaProvider"
    }

    fn version(&self) -> u32 {
        1
    }

    fn is_memory_suitable(&self, ptr: *const u8, size: usize) -> bool {
        // Mock accepts any non-empty buffer so RDMA paths can be exercised
        // without hardware or large allocations.
        let suitable = !ptr.is_null() && size > 0;
        self.dbg(&format!("is_memory_suitable ptr={ptr:p} size={size} -> {suitable}"));
        suitable
    }

    fn register_memory(&self, ptr: *mut u8, size: usize) -> Result<(), RdmaError> {
        self.dbg(&format!("register_memory ptr={ptr:p} size={size}"));
        self.registered.lock().unwrap().insert(ptr as usize, size);
        Ok(())
    }

    fn deregister_memory(&self, ptr: *mut u8) -> Result<(), RdmaError> {
        self.dbg(&format!("deregister_memory ptr={ptr:p}"));
        self.registered.lock().unwrap().remove(&(ptr as usize));
        Ok(())
    }

    fn prepare_put_token(
        &self,
        s3_key: &[u8],
        buffer: *const u8,
        size: usize,
        offset: usize,
    ) -> Result<Vec<u8>, RdmaError> {
        let key = String::from_utf8_lossy(s3_key);
        let token = format!("RDMA_PUT_{buffer:p}_{size}_{offset}_key_{key}").into_bytes();
        self.dbg(&format!(
            "prepare_put_token key={key} token={}",
            String::from_utf8_lossy(&token)
        ));
        Ok(token)
    }

    fn prepare_get_token(
        &self,
        s3_key: &[u8],
        buffer: *mut u8,
        size: usize,
        offset: usize,
    ) -> Result<Vec<u8>, RdmaError> {
        let key = String::from_utf8_lossy(s3_key);
        let token =
            format!("RDMA_GET_{buffer:p}_{size}_{offset}_key_{key}").into_bytes();
        self.dbg(&format!(
            "prepare_get_token key={key} token={}",
            String::from_utf8_lossy(&token)
        ));
        Ok(token)
    }

    fn process_reply_token(
        &self,
        reply_token: &[u8],
        callback: RdmaCompletionCallback,
    ) -> Result<(), RdmaError> {
        self.dbg(&format!(
            "process_reply_token token={}",
            String::from_utf8_lossy(reply_token)
        ));
        callback(RdmaCompletionResult {
            error_code: 0,
            reply_token: reply_token.to_vec(),
        });
        Ok(())
    }

    fn rdma_token_header_name(&self) -> &[u8] { self.token_header.as_bytes() }
    fn rdma_reply_header_name(&self) -> &[u8] { self.reply_header.as_bytes() }
}

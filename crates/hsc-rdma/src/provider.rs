use crate::error::RdmaError;

// ── Completion callback ──────────────────────────────────────────────────────

/// Result delivered when an RDMA operation completes.
#[derive(Debug)]
pub struct RdmaCompletionResult {
    /// 0 on success; non-zero error code on failure.
    pub error_code: i32,
    /// Reply token echoed back from the server (may be empty).
    #[allow(dead_code)]
    pub reply_token: Vec<u8>,
}

/// Callback invoked when an async RDMA operation finishes.
pub type RdmaCompletionCallback = Box<dyn FnOnce(RdmaCompletionResult) + Send + 'static>;

// ── Default header names ─────────────────────────────────────────────────────

pub const RDMA_TOKEN_HEADER: &str = "x-amz-rdma-token";
pub const RDMA_REPLY_HEADER: &str = "x-amz-rdma-reply";
#[allow(dead_code)]
pub const RDMA_BYTES_HEADER: &str = "x-amz-rdma-bytes";

// ── RdmaProvider trait ───────────────────────────────────────────────────────

/// Rust interface for an RDMA provider, modelled on the cuObject plugin vtable.
///
/// Implementations:
/// - [`MockRdmaProvider`](crate::mock::MockRdmaProvider) – no-hardware mock.
/// - [`CuObjectProvider`](crate::cuobject::CuObjectProvider) – real NVIDIA cuObject
///   bindings (requires the `cuobject` Cargo feature).
pub trait RdmaProvider: Send + Sync {
    /// Human-readable provider name.
    fn name(&self) -> &str;

    /// Numeric provider version.
    #[allow(dead_code)]
    fn version(&self) -> u32;

    /// Returns `true` if the memory region at `ptr` with `size` bytes is
    /// eligible for RDMA (e.g. CUDA device/pinned host memory).
    fn is_memory_suitable(&self, ptr: *const u8, size: usize) -> bool;

    /// Register a memory region so the RDMA subsystem can access it.
    ///
    /// # Safety
    /// `ptr` must remain valid until [`deregister_memory`](Self::deregister_memory).
    fn register_memory(&self, ptr: *mut u8, size: usize) -> Result<(), RdmaError>;

    /// Deregister a previously registered memory region.
    fn deregister_memory(&self, ptr: *mut u8) -> Result<(), RdmaError>;

    /// Maximum single-transfer size for the given buffer.
    /// Default: `usize::MAX` (no limit).  Override when the provider imposes a cap.
    #[allow(dead_code)]
    fn get_max_transfer_size(&self, _ptr: *const u8) -> usize {
        usize::MAX
    }

    /// Generate an RDMA descriptor token for a PUT (upload) sub-request.
    ///
    /// The returned bytes are used as the value of the `x-amz-rdma-token`
    /// request header.
    fn prepare_put_token(
        &self,
        s3_key: &[u8],
        buffer: *const u8,
        size: usize,
        offset: usize,
    ) -> Result<Vec<u8>, RdmaError>;

    /// Generate an RDMA descriptor token for a GET (download) sub-request.
    fn prepare_get_token(
        &self,
        s3_key: &[u8],
        buffer: *mut u8,
        size: usize,
        offset: usize,
    ) -> Result<Vec<u8>, RdmaError>;

    /// Process the `x-amz-rdma-reply` token returned by the S3 server.
    ///
    /// The provider confirms the RDMA transfer and invokes `callback` with
    /// the result.
    fn process_reply_token(
        &self,
        reply_token: &[u8],
        callback: RdmaCompletionCallback,
    ) -> Result<(), RdmaError>;

    // ── HTTP header names ────────────────────────────────────────────────────

    /// Header name for the outbound RDMA token (default: `x-amz-rdma-token`).
    fn rdma_token_header_name(&self) -> &[u8] {
        RDMA_TOKEN_HEADER.as_bytes()
    }

    /// Header name for the server RDMA reply token (default: `x-amz-rdma-reply`).
    fn rdma_reply_header_name(&self) -> &[u8] {
        RDMA_REPLY_HEADER.as_bytes()
    }

    /// Header name for the RDMA byte-count (default: `x-amz-rdma-bytes`).
    #[allow(dead_code)]
    fn rdma_bytes_header_name(&self) -> &[u8] {
        RDMA_BYTES_HEADER.as_bytes()
    }
}

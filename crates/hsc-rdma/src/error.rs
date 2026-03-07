use std::fmt;

/// Errors from RDMA provider operations.
#[allow(dead_code)]
#[derive(Debug)]
pub enum RdmaError {
    /// Memory registration failed.
    MemoryRegistrationFailed { ptr: usize, size: usize, reason: String },
    /// Memory deregistration failed.
    MemoryDeregistrationFailed { ptr: usize, reason: String },
    /// Memory is not suitable for RDMA.
    MemoryNotSuitable { ptr: usize, size: usize },
    /// Token generation failed.
    TokenGenerationFailed { key: String, reason: String },
    /// Reply token processing failed.
    ReplyTokenProcessingFailed(String),
    /// cuObject client failed to connect.
    CuObjectConnectionFailed(String),
    /// I/O error.
    Io(std::io::Error),
}

impl RdmaError {
    /// Returns `true` for errors where falling back to plain (non-RDMA) HTTP
    /// is safe — e.g. memory not suitable, token failure, library unavailable.
    ///
    /// Returns `false` for hard I/O or deregistration errors where the caller
    /// should propagate the failure.
    pub fn is_fallback_eligible(&self) -> bool {
        matches!(
            self,
            RdmaError::MemoryNotSuitable { .. }
                | RdmaError::TokenGenerationFailed { .. }
                | RdmaError::CuObjectConnectionFailed(_)
        )
    }
}

impl fmt::Display for RdmaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RdmaError::MemoryRegistrationFailed { ptr, size, reason } => {
                write!(f, "Failed to register memory at {ptr:#x} (size={size}): {reason}")
            }
            RdmaError::MemoryDeregistrationFailed { ptr, reason } => {
                write!(f, "Failed to deregister memory at {ptr:#x}: {reason}")
            }
            RdmaError::MemoryNotSuitable { ptr, size } => {
                write!(f, "Memory at {ptr:#x} (size={size}) not suitable for RDMA")
            }
            RdmaError::TokenGenerationFailed { key, reason } => {
                write!(f, "Failed to generate RDMA token for key '{key}': {reason}")
            }
            RdmaError::ReplyTokenProcessingFailed(msg) => {
                write!(f, "Failed to process RDMA reply token: {msg}")
            }
            RdmaError::CuObjectConnectionFailed(msg) => {
                write!(f, "cuObject client failed to connect: {msg}")
            }
            RdmaError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for RdmaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RdmaError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RdmaError {
    fn from(e: std::io::Error) -> Self {
        RdmaError::Io(e)
    }
}

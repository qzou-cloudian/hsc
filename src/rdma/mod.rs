//! Re-export [`s3_rdma`] as the local `rdma` module, plus the S3-specific
//! Smithy interceptor that lives here in the root crate.
pub use s3_rdma::*;

pub mod interceptor;
pub use interceptor::RdmaInterceptor;

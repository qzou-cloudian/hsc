//! cuObject RDMA provider plugin for hsc.
//!
//! This crate compiles `hsc-rdma` (with `cuobject` + `c-abi` features) into a
//! shared library that exposes the provider vtable with the standard C ABI.
//!
//! Exported symbol:
//! ```c
//! const RdmaProviderVtable *hsc_rdma_provider_get_vtable(void);
//! ```

use hsc_rdma::ffi::RdmaProviderVtable;

/// C ABI entry point: returns the statically allocated RDMA provider vtable.
#[no_mangle]
pub extern "C" fn hsc_rdma_provider_get_vtable() -> *const RdmaProviderVtable {
    hsc_rdma::get_vtable_ptr()
}

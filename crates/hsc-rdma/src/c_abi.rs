//! C-ABI export – makes `hsc-rdma` loadable as a plugin from C code.
//!
//! When `hsc-rdma` is compiled as a `cdylib` (with the `c-abi` feature), the
//! resulting `.so` exports:
//! ```c
//! const RdmaProviderVtable *hsc_rdma_provider_get_vtable(void);
//! ```
//!
//! The vtable is wired to the best available [`RdmaProvider`]:
//! `CuObjectProvider` when the `cuobject` feature is also enabled,
//! otherwise [`MockRdmaProvider`].

use std::ffi::c_void;
use std::os::raw::c_char;
use std::sync::OnceLock;

use crate::ffi::{ByteCursor, RdmaCompletionFn, RdmaProviderVtable};
use crate::provider::{RdmaCompletionCallback, RdmaProvider};

// ─── Provider name ────────────────────────────────────────────────────────────

static PROVIDER_NAME: &[u8] = b"hsc-rdma\0";

// ─── Singleton provider ───────────────────────────────────────────────────────

fn get_provider() -> &'static dyn RdmaProvider {
    static INSTANCE: OnceLock<Box<dyn RdmaProvider + Send + Sync>> = OnceLock::new();
    // When compiled as a cdylib plugin (c-abi feature), we must NOT attempt to
    // load CuObjectProvider here — that would dlopen the plugin itself, causing
    // infinite recursion.  The mock provider is the correct in-process backend;
    // callers that need real cuObject hardware wire it up via their own vtable.
    INSTANCE.get_or_init(|| {
        Box::new(crate::MockRdmaProvider::new(false))
    }).as_ref()
}

// ─── C shim functions ─────────────────────────────────────────────────────────

unsafe extern "C" fn c_init(_: *mut c_void, _: *mut *mut c_void) -> i32 { 0 }
unsafe extern "C" fn c_cleanup(_: *mut c_void) {}

unsafe extern "C" fn c_is_memory_suitable(_: *mut c_void, ptr: *const c_void, size: usize) -> bool {
    get_provider().is_memory_suitable(ptr as *const u8, size)
}

unsafe extern "C" fn c_register_memory(_: *mut c_void, ptr: *mut c_void, size: usize) -> i32 {
    match get_provider().register_memory(ptr as *mut u8, size) {
        Ok(()) => 0,
        Err(e) => { eprintln!("[hsc-rdma] register_memory: {e}"); -1 }
    }
}

unsafe extern "C" fn c_deregister_memory(_: *mut c_void, ptr: *mut c_void) -> i32 {
    match get_provider().deregister_memory(ptr as *mut u8) {
        Ok(()) => 0,
        Err(e) => { eprintln!("[hsc-rdma] deregister_memory: {e}"); -1 }
    }
}

unsafe extern "C" fn c_get_max_transfer_size(_: *mut c_void, ptr: *const c_void) -> usize {
    get_provider().get_max_transfer_size(ptr as *const u8)
}

unsafe extern "C" fn c_prepare_put_token(
    _: *mut c_void, s3_key: *const ByteCursor,
    buffer: *const c_void, size: usize, offset: usize,
    out: *mut ByteCursor,
) -> i32 {
    if s3_key.is_null() || out.is_null() { return -1; }
    let key = unsafe { (*s3_key).as_slice() };
    match get_provider().prepare_put_token(key, buffer as *const u8, size, offset) {
        Ok(token) => {
            let leaked = token.into_boxed_slice();
            let len = leaked.len();
            let ptr = Box::into_raw(leaked) as *const u8;
            unsafe { *out = ByteCursor { ptr, len }; }
            0
        }
        Err(e) => { eprintln!("[hsc-rdma] prepare_put_token: {e}"); -1 }
    }
}

unsafe extern "C" fn c_prepare_get_token(
    _: *mut c_void, s3_key: *const ByteCursor,
    buffer: *mut c_void, size: usize, offset: usize,
    out: *mut ByteCursor,
) -> i32 {
    if s3_key.is_null() || out.is_null() { return -1; }
    let key = unsafe { (*s3_key).as_slice() };
    match get_provider().prepare_get_token(key, buffer as *mut u8, size, offset) {
        Ok(token) => {
            let leaked = token.into_boxed_slice();
            let len = leaked.len();
            let ptr = Box::into_raw(leaked) as *const u8;
            unsafe { *out = ByteCursor { ptr, len }; }
            0
        }
        Err(e) => { eprintln!("[hsc-rdma] prepare_get_token: {e}"); -1 }
    }
}

unsafe extern "C" fn c_process_reply_token(
    _: *mut c_void, reply: *const ByteCursor,
    user_data: *mut c_void, callback: Option<RdmaCompletionFn>,
) -> i32 {
    if reply.is_null() { return -1; }
    let token = unsafe { (*reply).as_slice() }.to_vec();
    let ud = user_data as usize;

    let cb: RdmaCompletionCallback = Box::new(move |result| {
        if let Some(f) = callback {
            let reply_bytes = result.reply_token;
            let cursor = if reply_bytes.is_empty() {
                ByteCursor::null()
            } else {
                let leaked = reply_bytes.into_boxed_slice();
                let len = leaked.len();
                let ptr = Box::into_raw(leaked) as *const u8;
                ByteCursor { ptr, len }
            };
            unsafe { f(ud as *mut c_void, result.error_code, &cursor); }
        }
    });

    match get_provider().process_reply_token(&token, cb) {
        Ok(()) => 0,
        Err(e) => { eprintln!("[hsc-rdma] process_reply_token: {e}"); -1 }
    }
}

unsafe extern "C" fn c_rdma_token_header_name(_: *mut c_void) -> ByteCursor {
    let s = get_provider().rdma_token_header_name();
    ByteCursor { ptr: s.as_ptr(), len: s.len() }
}

unsafe extern "C" fn c_rdma_reply_header_name(_: *mut c_void) -> ByteCursor {
    let s = get_provider().rdma_reply_header_name();
    ByteCursor { ptr: s.as_ptr(), len: s.len() }
}

unsafe extern "C" fn c_rdma_bytes_header_name(_: *mut c_void) -> ByteCursor {
    let s = get_provider().rdma_bytes_header_name();
    ByteCursor { ptr: s.as_ptr(), len: s.len() }
}

// ─── Static vtable ────────────────────────────────────────────────────────────

static VTABLE: RdmaProviderVtable = RdmaProviderVtable {
    provider_name:    PROVIDER_NAME.as_ptr() as *const c_char,
    provider_version: 1,
    init:    Some(c_init),
    cleanup: Some(c_cleanup),
    is_memory_suitable:     Some(c_is_memory_suitable),
    register_memory:        Some(c_register_memory),
    deregister_memory:      Some(c_deregister_memory),
    get_max_transfer_size:  Some(c_get_max_transfer_size),
    prepare_put_token:      Some(c_prepare_put_token),
    prepare_get_token:      Some(c_prepare_get_token),
    process_reply_token:    Some(c_process_reply_token),
    rdma_token_header_name: Some(c_rdma_token_header_name),
    rdma_reply_header_name: Some(c_rdma_reply_header_name),
    rdma_bytes_header_name: Some(c_rdma_bytes_header_name),
};

/// Internal entry point: returns a pointer to the static vtable.
///
/// The cdylib crate (`hsc-rdma-cuobj`) calls this and re-exports it as
/// `#[no_mangle] hsc_rdma_provider_get_vtable`, keeping the symbol out of
/// `hsc-rdma` when compiled as an rlib.
pub(crate) fn get_vtable() -> *const RdmaProviderVtable {
    &VTABLE
}

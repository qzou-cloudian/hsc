//! NVIDIA cuObject RDMA provider via lazy runtime loading.
//!
//! Enabled by the `cuobject` Cargo feature.  At runtime `hsc` searches for the
//! provider plugin (`libhsc_rdma_cuobj.so`) and loads it on demand.  The main
//! `hsc` binary therefore has **no compile-time or ELF-level dependency** on
//! `libcuobjclient.so` – the library is pulled in transitively when the plugin
//! is dlopened.
//!
//! Search order for `libhsc_rdma_cuobj.so`:
//! 1. Standard dynamic-linker paths (`LD_LIBRARY_PATH`, RPATH, ldconfig cache).
//! 2. Directory containing the current executable.
//! 3. `/lib64/`, `/usr/lib64/` (common CUDA/cuObject install locations).

#[cfg(feature = "cuobject")]
mod real {
    use std::ffi::c_void;

    use libloading::{Library, Symbol};

    use crate::error::RdmaError;
    use crate::ffi::{ByteCursor, GetVtableFn, RdmaCompletionFn, RdmaProviderVtable};
    use crate::provider::{RdmaCompletionCallback, RdmaCompletionResult, RdmaProvider};

    // ── Library search ────────────────────────────────────────────────────────

    const LIB_NAME: &str = "libhsc_rdma_cuobj.so";

    fn try_load(path: impl AsRef<std::ffi::OsStr>) -> Option<Library> {
        unsafe { Library::new(path) }.ok()
    }

    fn find_library() -> Option<Library> {
        // 1. Standard linker search (LD_LIBRARY_PATH, RPATH, ldconfig)
        if let Some(lib) = try_load(LIB_NAME) {
            return Some(lib);
        }
        // 2. Alongside the running executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                if let Some(lib) = try_load(dir.join(LIB_NAME)) {
                    return Some(lib);
                }
            }
        }
        // 3. Common CUDA/cuObject install directories
        for dir in &["/lib64", "/usr/lib64"] {
            if let Some(lib) = try_load(format!("{dir}/{LIB_NAME}")) {
                return Some(lib);
            }
        }
        None
    }

    // ── Completion trampoline ─────────────────────────────────────────────────

    /// Static C callback used with `process_reply_token`.
    ///
    /// The provider implementations call this synchronously, so `user_data`
    /// points to a live `Box<RdmaCompletionCallback>` on the Rust side.
    unsafe extern "C" fn completion_trampoline(
        user_data:   *mut c_void,
        error_code:  i32,
        reply_token: *const ByteCursor,
    ) {
        let cb = unsafe { Box::from_raw(user_data as *mut RdmaCompletionCallback) };
        let token = if reply_token.is_null() {
            vec![]
        } else {
            unsafe { (*reply_token).as_slice().to_vec() }
        };
        cb(RdmaCompletionResult { error_code, reply_token: token });
    }

    // ── CuObjectProvider ─────────────────────────────────────────────────────

    /// RDMA provider that loads `libhsc_rdma_cuobj.so` at runtime and calls
    /// through the exported [`RdmaProviderVtable`].
    pub struct CuObjectProvider {
        // Order matters: ctx/vtable must be dropped (logically) before _lib
        // unloads, so cleanup() is called in Drop::drop() before _lib drops.
        vtable: *const RdmaProviderVtable,
        ctx:    *mut c_void,
        _lib:   Library,  // keep the .so mapped; drops last
    }

    unsafe impl Send for CuObjectProvider {}
    unsafe impl Sync for CuObjectProvider {}

    impl std::fmt::Debug for CuObjectProvider {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "CuObjectProvider(vtable)")
        }
    }

    impl CuObjectProvider {
        /// Try to load the provider plugin and initialise it.
        ///
        /// Returns `Err` if the plugin is not found, fails to load (e.g.
        /// `libcuobjclient.so` is missing), or the vtable's `init` fails.
        pub fn new() -> Result<Self, RdmaError> {
            let lib = find_library().ok_or_else(|| {
                RdmaError::CuObjectConnectionFailed(
                    format!("{LIB_NAME} not found (searched LD_LIBRARY_PATH, exe dir, /lib64)")
                )
            })?;

            let vtable: *const RdmaProviderVtable = unsafe {
                let get_vtable: Symbol<GetVtableFn> = lib
                    .get(b"hsc_rdma_provider_get_vtable\0")
                    .map_err(|e| RdmaError::CuObjectConnectionFailed(e.to_string()))?;
                let ptr = get_vtable();
                if ptr.is_null() {
                    return Err(RdmaError::CuObjectConnectionFailed(
                        "hsc_rdma_provider_get_vtable returned null".into()
                    ));
                }
                ptr
            };

            let mut ctx = std::ptr::null_mut::<c_void>();
            if let Some(init_fn) = unsafe { (*vtable).init } {
                let rc = unsafe { init_fn(std::ptr::null_mut(), &mut ctx) };
                if rc != 0 {
                    return Err(RdmaError::CuObjectConnectionFailed(
                        format!("vtable init() returned {rc}")
                    ));
                }
            }

            Ok(Self { vtable, ctx, _lib: lib })
        }

        fn vtable(&self) -> &RdmaProviderVtable {
            // SAFETY: vtable lives in the library's data segment which is
            // mapped for the lifetime of `self` (kept alive by `_lib`).
            unsafe { &*self.vtable }
        }
    }

    impl Drop for CuObjectProvider {
        fn drop(&mut self) {
            // Call cleanup before `_lib` drops and unloads the .so.
            if let Some(cleanup) = self.vtable().cleanup {
                unsafe { cleanup(self.ctx); }
            }
        }
    }

    // ── RdmaProvider impl ─────────────────────────────────────────────────────

    impl RdmaProvider for CuObjectProvider {
        fn name(&self)    -> &str { "CuObjectProvider" }
        fn version(&self) -> u32  { unsafe { (*self.vtable).provider_version } }

        fn is_memory_suitable(&self, ptr: *const u8, size: usize) -> bool {
            let Some(f) = self.vtable().is_memory_suitable else { return false; };
            unsafe { f(self.ctx, ptr as *const c_void, size) }
        }

        fn register_memory(&self, ptr: *mut u8, size: usize) -> Result<(), RdmaError> {
            let Some(f) = self.vtable().register_memory else { return Ok(()); };
            let rc = unsafe { f(self.ctx, ptr as *mut c_void, size) };
            if rc != 0 {
                Err(RdmaError::MemoryRegistrationFailed {
                    ptr: ptr as usize, size,
                    reason: format!("vtable register_memory returned {rc}"),
                })
            } else { Ok(()) }
        }

        fn deregister_memory(&self, ptr: *mut u8) -> Result<(), RdmaError> {
            let Some(f) = self.vtable().deregister_memory else { return Ok(()); };
            let rc = unsafe { f(self.ctx, ptr as *mut c_void) };
            if rc != 0 {
                Err(RdmaError::MemoryDeregistrationFailed {
                    ptr: ptr as usize,
                    reason: format!("vtable deregister_memory returned {rc}"),
                })
            } else { Ok(()) }
        }

        fn get_max_transfer_size(&self, ptr: *const u8) -> usize {
            let Some(f) = self.vtable().get_max_transfer_size else { return usize::MAX; };
            let v = unsafe { f(self.ctx, ptr as *const c_void) };
            if v == 0 { usize::MAX } else { v }
        }

        fn prepare_put_token(
            &self, s3_key: &[u8], buffer: *const u8, size: usize, offset: usize,
        ) -> Result<Vec<u8>, RdmaError> {
            let Some(f) = self.vtable().prepare_put_token else {
                return Err(RdmaError::TokenGenerationFailed {
                    key: String::from_utf8_lossy(s3_key).into_owned(),
                    reason: "prepare_put_token not implemented".into(),
                });
            };
            let key_cursor = ByteCursor::from_slice(s3_key);
            let mut out = ByteCursor::null();
            let rc = unsafe {
                f(self.ctx, &key_cursor, buffer as *const c_void, size, offset, &mut out)
            };
            if rc != 0 {
                return Err(RdmaError::TokenGenerationFailed {
                    key: String::from_utf8_lossy(s3_key).into_owned(),
                    reason: format!("vtable prepare_put_token returned {rc}"),
                });
            }
            // SAFETY: the cdylib allocates the token bytes with Box::into_raw.
            let token = unsafe {
                let slice = std::slice::from_raw_parts(out.ptr, out.len);
                let v = slice.to_vec();
                // Reclaim the box the cdylib leaked.
                drop(Box::from_raw(
                    std::ptr::slice_from_raw_parts_mut(out.ptr as *mut u8, out.len)
                ));
                v
            };
            Ok(token)
        }

        fn prepare_get_token(
            &self, s3_key: &[u8], buffer: *mut u8, size: usize, offset: usize,
        ) -> Result<Vec<u8>, RdmaError> {
            let Some(f) = self.vtable().prepare_get_token else {
                return Err(RdmaError::TokenGenerationFailed {
                    key: String::from_utf8_lossy(s3_key).into_owned(),
                    reason: "prepare_get_token not implemented".into(),
                });
            };
            let key_cursor = ByteCursor::from_slice(s3_key);
            let mut out = ByteCursor::null();
            let rc = unsafe {
                f(self.ctx, &key_cursor, buffer as *mut c_void, size, offset, &mut out)
            };
            if rc != 0 {
                return Err(RdmaError::TokenGenerationFailed {
                    key: String::from_utf8_lossy(s3_key).into_owned(),
                    reason: format!("vtable prepare_get_token returned {rc}"),
                });
            }
            let token = unsafe {
                let slice = std::slice::from_raw_parts(out.ptr, out.len);
                let v = slice.to_vec();
                drop(Box::from_raw(
                    std::ptr::slice_from_raw_parts_mut(out.ptr as *mut u8, out.len)
                ));
                v
            };
            Ok(token)
        }

        fn process_reply_token(
            &self, reply_token: &[u8], callback: RdmaCompletionCallback,
        ) -> Result<(), RdmaError> {
            let Some(f) = self.vtable().process_reply_token else {
                // No-op: just fire the callback with success.
                callback(RdmaCompletionResult { error_code: 0, reply_token: reply_token.to_vec() });
                return Ok(());
            };
            // Box the callback and pass it as user_data to the trampoline.
            // The trampoline reclaims the Box; if rc != 0 we reclaim it here.
            let cb_ptr = Box::into_raw(Box::new(callback));
            let reply_cursor = ByteCursor::from_slice(reply_token);
            let rc = unsafe {
                f(self.ctx, &reply_cursor, cb_ptr as *mut c_void,
                  Some(completion_trampoline as RdmaCompletionFn))
            };
            if rc != 0 {
                // Callback was not invoked; reclaim the box.
                unsafe { drop(Box::from_raw(cb_ptr)); }
                Err(RdmaError::ReplyTokenProcessingFailed(
                    format!("vtable process_reply_token returned {rc}")
                ))
            } else {
                Ok(())
            }
        }

        fn rdma_token_header_name(&self) -> &[u8] {
            let Some(f) = self.vtable().rdma_token_header_name else {
                return crate::provider::RDMA_TOKEN_HEADER.as_bytes();
            };
            let cursor = unsafe { f(self.ctx) };
            if cursor.ptr.is_null() {
                return crate::provider::RDMA_TOKEN_HEADER.as_bytes();
            }
            // SAFETY: returned ByteCursor points into static string in the .so.
            unsafe { std::slice::from_raw_parts(cursor.ptr, cursor.len) }
        }

        fn rdma_reply_header_name(&self) -> &[u8] {
            let Some(f) = self.vtable().rdma_reply_header_name else {
                return crate::provider::RDMA_REPLY_HEADER.as_bytes();
            };
            let cursor = unsafe { f(self.ctx) };
            if cursor.ptr.is_null() {
                return crate::provider::RDMA_REPLY_HEADER.as_bytes();
            }
            unsafe { std::slice::from_raw_parts(cursor.ptr, cursor.len) }
        }
    }

    // ── Availability probe ────────────────────────────────────────────────────

    /// Returns `true` if the cuObject provider plugin can be loaded
    /// (i.e., `libhsc_rdma_cuobj.so` and `libcuobjclient.so` are installed).
    pub fn is_available() -> bool {
        CuObjectProvider::new().is_ok()
    }
}

#[cfg(feature = "cuobject")]
pub use real::CuObjectProvider;
#[cfg(feature = "cuobject")]
pub use real::is_available as cuobject_available;


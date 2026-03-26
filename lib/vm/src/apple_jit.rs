// Apple aarch64 JIT memory model (required for iOS and macOS Hardened Runtime):
// - Code pages are allocated with MAP_JIT (in mmap.rs)
// - Pages start in writable mode (PROT_READ | PROT_WRITE)
// - Before execution, pthread_jit_write_protect_np(1) toggles to executable
// - Before writing new code, pthread_jit_write_protect_np(0) toggles back
// - This toggle is per-thread, so each thread must manage its own state
// - After toggling to executable, sys_icache_invalidate flushes the icache
//
// MAP_JIT is only used when the traditional mprotect path doesn't work:
// - On iOS, mprotect to PROT_EXEC is always blocked
// - On macOS Hardened Runtime without allow-unsigned-executable-memory,
//   mprotect to PROT_EXEC is blocked (allow-jit only enables MAP_JIT)
// - On macOS development builds (no Hardened Runtime), mprotect works
//   and MAP_JIT + pthread_jit_write_protect_np may not function correctly

use std::sync::OnceLock;

type JitWriteProtectFn = unsafe extern "C" fn(libc::c_int);

static JIT_WRITE_PROTECT: OnceLock<Option<JitWriteProtectFn>> = OnceLock::new();
static MAP_JIT_SUPPORTED: OnceLock<bool> = OnceLock::new();

fn write_protect_fn() -> Option<JitWriteProtectFn> {
    *JIT_WRITE_PROTECT.get_or_init(|| unsafe {
        let sym = libc::dlsym(
            libc::RTLD_DEFAULT,
            b"pthread_jit_write_protect_np\0".as_ptr() as *const _,
        );
        if sym.is_null() {
            None
        } else {
            Some(std::mem::transmute(sym))
        }
    })
}

/// Whether the MAP_JIT + pthread_jit_write_protect_np mechanism is needed
/// and available. Returns true only when mprotect to PROT_EXEC is blocked
/// (iOS, macOS Hardened Runtime) and the MAP_JIT API exists.
pub fn is_supported() -> bool {
    *MAP_JIT_SUPPORTED.get_or_init(|| {
        // Need the API to be available
        if write_protect_fn().is_none() {
            return false;
        }

        // Probe whether mprotect to PROT_EXEC works. If it does, we don't
        // need MAP_JIT — the traditional path is simpler and more reliable.
        !can_mprotect_exec()
    })
}

/// Test whether mprotect can make anonymous pages executable.
/// Returns false on iOS and macOS Hardened Runtime (without
/// allow-unsigned-executable-memory), where MAP_JIT is required.
fn can_mprotect_exec() -> bool {
    unsafe {
        let page_size = region::page::size();
        let ptr = libc::mmap(
            std::ptr::null_mut(),
            page_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        if ptr == libc::MAP_FAILED {
            return false;
        }
        let result = libc::mprotect(ptr, page_size, libc::PROT_READ | libc::PROT_EXEC);
        libc::munmap(ptr, page_size);
        result == 0
    }
}

/// Switch MAP_JIT pages to writable (not executable) for the current thread.
/// No-op if MAP_JIT is not needed on this platform.
pub fn enable_write() {
    if !is_supported() {
        return;
    }
    if let Some(f) = write_protect_fn() {
        unsafe { f(0) };
    }
}

/// Switch MAP_JIT pages to executable (not writable) for the current thread
/// and flush the instruction cache for the given range. No-op if MAP_JIT
/// is not needed on this platform.
pub unsafe fn enable_execute(ptr: *mut u8, len: usize) {
    if !is_supported() {
        return;
    }

    unsafe extern "C" {
        fn sys_icache_invalidate(start: *mut libc::c_void, size: libc::size_t);
    }

    if let Some(f) = write_protect_fn() {
        unsafe { f(1) };
        unsafe {
            sys_icache_invalidate(ptr as *mut libc::c_void, len);
        }
    }
}

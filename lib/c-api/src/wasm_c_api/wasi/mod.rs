//! Unofficial API for WASI integrating with the standard Wasm C API.
//!
//! This API will be superseded by a standard WASI API when/if such a standard is created.

pub use super::unstable::wasi::wasi_get_unordered_imports;
use super::{
    externals::{wasm_extern_t, wasm_extern_vec_t, wasm_func_t, wasm_memory_t},
    instance::wasm_instance_t,
    module::wasm_module_t,
    store::{StoreRef, wasm_store_t},
    types::wasm_byte_vec_t,
};
use crate::error::update_last_error;
use std::convert::TryFrom;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::slice;
use std::sync::Arc;
#[cfg(feature = "webc_runner")]
use wasmer_api::{AsStoreMut, Imports, Module};
use wasmer_api::Memory;
use wasmer_wasix::{
    Pipe, PluggableRuntime, WasiEnv, WasiEnvBuilder, WasiFunctionEnv, WasiVersion,
    default_fs_backing, get_wasi_version,
    runtime::task_manager::{tokio::TokioTaskManager, block_on},
};
use wasmer_types::ModuleHash;
use std::path::PathBuf;
use wasmer_wasix::virtual_fs::FileSystem as _;

#[derive(Debug)]
#[allow(non_camel_case_types)]
pub struct wasi_config_t {
    inherit_stdout: bool,
    inherit_stderr: bool,
    inherit_stdin: bool,
    builder: WasiEnvBuilder,
    runtime: Option<tokio::runtime::Runtime>,
    /// Mapped directories: (guest_alias, host_path).
    /// These are tracked separately so we can set up filesystem mounts
    /// in wasi_env_new, making the mapped directories visible to BinFactory
    /// for proc_exec (running external WASM binaries from within bash).
    mapped_dirs: Vec<(String, PathBuf)>,
    /// Whether wasi_config_preopen_dir was called. Preopened dirs are
    /// incompatible with mapped dirs (which use a TmpFileSystem sandbox),
    /// so wasi_env_new returns an error if both are used.
    has_preopen_dirs: bool,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_config_new(
    program_name: *const c_char,
) -> Option<Box<wasi_config_t>> {
    debug_assert!(!program_name.is_null());

    let name_c_str = unsafe { CStr::from_ptr(program_name) };
    let prog_name = c_try!(name_c_str.to_str());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = runtime.enter();

    Some(Box::new(wasi_config_t {
        inherit_stdout: true,
        inherit_stderr: true,
        inherit_stdin: true,
        builder: WasiEnv::builder(prog_name),
        runtime: Some(runtime),
        mapped_dirs: Vec::new(),
        has_preopen_dirs: false,
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_config_env(
    config: &mut wasi_config_t,
    key: *const c_char,
    value: *const c_char,
) {
    debug_assert!(!key.is_null());
    debug_assert!(!value.is_null());

    let key_cstr = unsafe { CStr::from_ptr(key) };
    let key_bytes = key_cstr.to_bytes();
    let value_cstr = unsafe { CStr::from_ptr(value) };
    let value_bytes = value_cstr.to_bytes();

    config.builder.add_env(key_bytes, value_bytes);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_config_arg(config: &mut wasi_config_t, arg: *const c_char) {
    debug_assert!(!arg.is_null());

    let arg_cstr = unsafe { CStr::from_ptr(arg) };
    let arg_bytes = arg_cstr.to_bytes();

    config.builder.add_arg(arg_bytes);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_config_preopen_dir(
    config: &mut wasi_config_t,
    dir: *const c_char,
) -> bool {
    let dir_cstr = unsafe { CStr::from_ptr(dir) };
    let dir_bytes = dir_cstr.to_bytes();
    let dir_str = match std::str::from_utf8(dir_bytes) {
        Ok(dir_str) => dir_str,
        Err(e) => {
            update_last_error(e);
            return false;
        }
    };

    if let Err(e) = config.builder.add_preopen_dir(dir_str) {
        update_last_error(e);
        return false;
    }

    config.has_preopen_dirs = true;
    true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_config_mapdir(
    config: &mut wasi_config_t,
    alias: *const c_char,
    dir: *const c_char,
) -> bool {
    let alias_cstr = unsafe { CStr::from_ptr(alias) };
    let alias_bytes = alias_cstr.to_bytes();
    let alias_str = match std::str::from_utf8(alias_bytes) {
        Ok(alias_str) => alias_str,
        Err(e) => {
            update_last_error(e);
            return false;
        }
    };

    let dir_cstr = unsafe { CStr::from_ptr(dir) };
    let dir_bytes = dir_cstr.to_bytes();
    let dir_str = match std::str::from_utf8(dir_bytes) {
        Ok(dir_str) => dir_str,
        Err(e) => {
            update_last_error(e);
            return false;
        }
    };

    // Record the mapping for filesystem setup in wasi_env_new.
    // The alias is stored as-is; normalization happens during setup.
    config.mapped_dirs.push((alias_str.to_string(), PathBuf::from(dir_str)));

    true
}

#[unsafe(no_mangle)]
pub extern "C" fn wasi_config_capture_stdout(config: &mut wasi_config_t) {
    config.inherit_stdout = false;
}

#[unsafe(no_mangle)]
pub extern "C" fn wasi_config_inherit_stdout(config: &mut wasi_config_t) {
    config.inherit_stdout = true;
}

#[unsafe(no_mangle)]
pub extern "C" fn wasi_config_capture_stderr(config: &mut wasi_config_t) {
    config.inherit_stderr = false;
}

#[unsafe(no_mangle)]
pub extern "C" fn wasi_config_inherit_stderr(config: &mut wasi_config_t) {
    config.inherit_stderr = true;
}

#[unsafe(no_mangle)]
pub extern "C" fn wasi_config_capture_stdin(config: &mut wasi_config_t) {
    config.inherit_stdin = false;
}

#[unsafe(no_mangle)]
pub extern "C" fn wasi_config_inherit_stdin(config: &mut wasi_config_t) {
    config.inherit_stdin = true;
}

#[repr(C)]
pub struct wasi_filesystem_t {
    ptr: *const c_char,
    size: usize,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_filesystem_init_static_memory(
    volume_bytes: Option<&wasm_byte_vec_t>,
) -> Option<Box<wasi_filesystem_t>> {
    let volume_bytes = volume_bytes.as_ref()?;
    Some(Box::new(wasi_filesystem_t {
        ptr: {
            let ptr = unsafe { volume_bytes.data.as_ref()? } as *const _ as *const c_char;
            if ptr.is_null() {
                return None;
            }
            ptr
        },
        size: volume_bytes.size,
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_filesystem_delete(_ptr: Option<Box<wasi_filesystem_t>>) {}

/// Initializes the `imports` with an import object that links to
/// the custom file system
#[cfg(feature = "webc_runner")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_with_filesystem(
    config: Box<wasi_config_t>,
    store: Option<&mut wasm_store_t>,
    module: Option<&wasm_module_t>,
    fs: Option<&wasi_filesystem_t>,
    imports: Option<&mut wasm_extern_vec_t>,
    package: *const c_char,
) -> Option<Box<wasi_env_t>> {
    unsafe { wasi_env_with_filesystem_inner(config, store, module, fs, imports, package) }
}

#[cfg(feature = "webc_runner")]
unsafe fn wasi_env_with_filesystem_inner(
    config: Box<wasi_config_t>,
    store: Option<&mut wasm_store_t>,
    module: Option<&wasm_module_t>,
    fs: Option<&wasi_filesystem_t>,
    imports: Option<&mut wasm_extern_vec_t>,
    package: *const c_char,
) -> Option<Box<wasi_env_t>> {
    let store = &mut store?.inner;
    let fs = fs.as_ref()?;
    let package_str = unsafe { CStr::from_ptr(package) };
    let package = package_str.to_str().unwrap_or("");
    let module = &module.as_ref()?.inner;
    let imports = imports?;
    #[allow(clippy::unnecessary_cast)]
    let fs_bytes = unsafe { &*(fs.ptr as *const u8) };

    let (wasi_env, import_object, stdout_rx, stderr_rx, stdin_tx) = {
        let mut store_mut = unsafe { store.store_mut() };
        prepare_webc_env(
            config,
            &mut store_mut,
            module,
            fs_bytes, // cast wasi_filesystem_t.ptr as &'static [u8]
            fs.size,
            package,
        )?
    };

    imports_set_buffer(store, module, import_object, imports)?;

    Some(Box::new(wasi_env_t {
        inner: wasi_env,
        store: store.clone(),
        stdout_rx,
        stderr_rx,
        stdin_tx,
        imported_memory: None,
        // TODO: pass the tokio runtime handle from prepare_webc_env so that
        // wasi_start can enter the runtime for this code path too
        runtime_handle: None,
        extra_imports: Vec::new(),
    }))
}

#[cfg(feature = "webc_runner")]
fn prepare_webc_env(
    mut config: Box<wasi_config_t>,
    store: &mut impl AsStoreMut,
    module: &Module,
    bytes: &'static u8,
    len: usize,
    package_name: &str,
) -> Option<(WasiFunctionEnv, Imports, Option<Pipe>, Option<Pipe>, Option<Pipe>)> {
    use virtual_fs::static_fs::StaticFileSystem;
    use wasmer_wasix::virtual_fs::FileSystem;
    use webc::v1::{FsEntryType, WebC};

    let store_mut = store.as_store_mut();
    let runtime = config.runtime.take();

    let runtime = runtime.unwrap_or_else(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    });

    let handle = runtime.handle().clone();
    let _guard = handle.enter();
    let mut rt = PluggableRuntime::new(Arc::new(TokioTaskManager::new(runtime)));
    rt.set_engine(store_mut.engine().clone());

    let slice = unsafe { std::slice::from_raw_parts(bytes, len) };
    let volumes = WebC::parse_volumes_from_fileblock(slice).ok()?;
    let top_level_dirs = volumes
        .into_iter()
        .flat_map(|(_, volume)| {
            volume
                .header
                .top_level
                .iter()
                .filter(|entry| entry.fs_type == FsEntryType::Dir)
                .map(|e| e.text.to_string())
                .collect::<Vec<_>>()
                .into_iter()
        })
        .collect::<Vec<_>>();

    let filesystem =
        Arc::new(StaticFileSystem::init(slice, package_name)?) as Arc<dyn FileSystem + Send + Sync>;
    let mut builder = config.builder.runtime(Arc::new(rt));

    let mut stdout_rx = None;
    if !config.inherit_stdout {
        let (stdout_tx, rx) = Pipe::channel();
        builder.set_stdout(Box::new(stdout_tx));
        stdout_rx = Some(rx);
    }

    let mut stderr_rx = None;
    if !config.inherit_stderr {
        let (stderr_tx, rx) = Pipe::channel();
        builder.set_stderr(Box::new(stderr_tx));
        stderr_rx = Some(rx);
    }

    let mut stdin_tx = None;
    if !config.inherit_stdin {
        let (tx, stdin_rx) = Pipe::channel();
        builder.set_stdin(Box::new(stdin_rx));
        stdin_tx = Some(tx);
    }

    builder.set_fs(filesystem);

    for f_name in top_level_dirs.iter() {
        builder
            .add_preopen_build(|p| p.directory(f_name).read(true).write(true).create(true))
            .ok()?;
    }
    let env = builder.finalize(store).ok()?;

    let import_object = env.import_object_for_all_wasi_versions(store, module).ok()?;
    Some((env, import_object, stdout_rx, stderr_rx, stdin_tx))
}

#[allow(non_camel_case_types)]
pub struct wasi_env_t {
    /// cbindgen:ignore
    pub(super) inner: WasiFunctionEnv,
    pub(super) store: StoreRef,
    /// Host-side read end for captured stdout
    stdout_rx: Option<Pipe>,
    /// Host-side read end for captured stderr
    stderr_rx: Option<Pipe>,
    /// Host-side write end for captured stdin
    stdin_tx: Option<Pipe>,
    /// Memory for WASIX modules that import rather than export memory
    imported_memory: Option<Memory>,
    /// Tokio runtime handle for entering the async context during execution.
    /// WASIX modules that use proc_fork need the tokio runtime to be active
    /// on the calling thread so that child tasks can be properly scheduled.
    runtime_handle: Option<tokio::runtime::Handle>,
    /// Custom host function imports to include alongside WASI imports.
    /// Each entry is (module_name, import_name, extern). These are merged
    /// into the import object when wasi_get_imports resolves imports.
    extra_imports: Vec<(String, String, wasmer_api::Extern)>,
}

/// Create a new WASI environment.
///
/// It take ownership over the `wasi_config_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_new(
    store: Option<&mut wasm_store_t>,
    mut config: Box<wasi_config_t>,
) -> Option<Box<wasi_env_t>> {
    let store = &mut store?.inner;
    let mut store_mut = unsafe { store.store_mut() };

    let runtime = config.runtime.take();

    let runtime = runtime.unwrap_or_else(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    });

    let handle = runtime.handle().clone();
    let _guard = handle.enter();
    let mut rt = PluggableRuntime::new(Arc::new(TokioTaskManager::new(runtime)));
    rt.set_engine(store_mut.engine().clone());

    let mut stdout_rx = None;
    if !config.inherit_stdout {
        let (stdout_tx, rx) = Pipe::channel();
        config.builder.set_stdout(Box::new(stdout_tx));
        stdout_rx = Some(rx);
    }

    let mut stderr_rx = None;
    if !config.inherit_stderr {
        let (stderr_tx, rx) = Pipe::channel();
        config.builder.set_stderr(Box::new(stderr_tx));
        stderr_rx = Some(rx);
    }

    let mut stdin_tx = None;
    if !config.inherit_stdin {
        let (tx, stdin_rx) = Pipe::channel();
        config.builder.set_stdin(Box::new(stdin_rx));
        stdin_tx = Some(tx);
    }

    // Set up the filesystem with mounts for mapped directories.
    // This uses a TmpFileSystem (like the CLI does) so that BinFactory can
    // find executables at guest alias paths (e.g., /tools/jq) when bash
    // invokes proc_exec to run external WASM binaries.
    if !config.mapped_dirs.is_empty() {
        if config.has_preopen_dirs {
            update_last_error(
                "wasi_config_preopen_dir and wasi_config_mapdir cannot be used together; \
                 mapped directories use a sandboxed filesystem that preopened host paths \
                 cannot resolve against"
            );
            return None;
        }

        // Normalize guest paths once: ensure leading "/" and pair with host dir.
        let normalized: Vec<(String, PathBuf)> = config.mapped_dirs.iter().map(|(alias, host_dir)| {
            let guest_path = if alias.starts_with('/') {
                alias.clone()
            } else {
                format!("/{}", alias)
            };
            (guest_path, host_dir.clone())
        }).collect();

        // Use build_ext to exclude mapped guest paths from the default dirs,
        // avoiding AlreadyExists errors if an alias matches /bin, /tmp, etc.
        let guest_path_refs: Vec<&str> = normalized.iter().map(|(g, _)| g.as_str()).collect();
        let root_fs = wasmer_wasix::virtual_fs::RootFileSystemBuilder::new()
            .build_ext(&guest_path_refs);
        let host_fs = default_fs_backing();

        for (guest_path, host_dir) in &normalized {
            // Create parent directories for nested guest paths.
            // mount() creates the final component, but parents must exist first.
            // Example: for /usr/local/bin, this creates /usr and /usr/local.
            let guest_pb = PathBuf::from(guest_path);
            if let Some(parent) = guest_pb.parent() {
                if parent != std::path::Path::new("/") {
                    let mut current = PathBuf::from("/");
                    for component in parent.components().skip(1) {
                        current.push(component);
                        let _ = root_fs.create_dir(&current);
                    }
                }
            }
            if let Err(e) = root_fs.mount(
                PathBuf::from(guest_path),
                &host_fs,
                host_dir.clone(),
            ) {
                update_last_error(format!("Failed to mount {} -> {:?}: {}", guest_path, host_dir, e));
                return None;
            }
            // Use add_map_dir with the GUEST path (not the host path) since root_fs
            // has a mount at the guest path that redirects to the host directory.
            // This makes the preopened dir's Kind::Dir { path } point to the guest
            // path, so both get_inode_at_path and BinFactory resolve through the mount.
            if let Err(e) = config.builder.add_map_dir(guest_path, guest_path) {
                update_last_error(e);
                return None;
            }
        }
        // sandbox_fs() and preopen_dir() consume self, so swap the builder out and back.
        let builder = std::mem::replace(&mut config.builder, WasiEnv::builder(""));
        let builder = c_try!(builder
            .sandbox_fs(root_fs)
            .preopen_dir(std::path::Path::new("/")));
        config.builder = c_try!(builder.map_dir(".", "/"));
    } else {
        config.builder.set_fs(default_fs_backing());
    }

    let env = c_try!(
        config
            .builder
            .runtime(Arc::new(rt))
            .finalize(&mut store_mut)
    );

    Some(Box::new(wasi_env_t {
        inner: env,
        store: store.clone(),
        stdout_rx,
        stderr_rx,
        stdin_tx,
        imported_memory: None,
        runtime_handle: Some(handle),
        extra_imports: Vec::new(),
    }))
}

/// Register a custom host function import on a [`wasi_env_t`].
///
/// When [`wasi_get_imports`] resolves imports for a module, any registered
/// extra imports are included alongside the standard WASI imports. This
/// allows modules with non-WASI imports (e.g., `env.exec_command`) to be
/// instantiated through the normal WASI import resolution path.
///
/// `module_name` and `import_name` are null-terminated C strings.
/// The function is cloned internally, so the caller retains ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_add_host_function(
    wasi_env: Option<&mut wasi_env_t>,
    module_name: *const c_char,
    import_name: *const c_char,
    func: Option<&wasm_func_t>,
) -> bool {
    let Some(wasi_env) = wasi_env else { return false };
    let Some(func) = func else { return false };
    if module_name.is_null() || import_name.is_null() {
        return false;
    }
    let module_name = unsafe { CStr::from_ptr(module_name) }
        .to_string_lossy()
        .into_owned();
    let import_name = unsafe { CStr::from_ptr(import_name) }
        .to_string_lossy()
        .into_owned();
    wasi_env
        .extra_imports
        .push((module_name, import_name, func.extern_.inner.clone()));
    true
}

/// Delete a [`wasi_env_t`].
#[unsafe(no_mangle)]
pub extern "C" fn wasi_env_delete(state: Option<Box<wasi_env_t>>) {
    if let Some(mut env) = state {
        let mut store = unsafe { env.store.store_mut() };
        env.inner.on_exit(&mut store, None);
    }
}

/// Set the memory on a [`wasi_env_t`].
// NOTE: Only here to not break the C API.
// This was previosly supported, but is no longer possible due to WASIX changes.
// Customizing memories should be done through the builder or the runtime.
#[unsafe(no_mangle)]
#[deprecated(since = "4.0.0")]
pub unsafe extern "C" fn wasi_env_set_memory(_env: &mut wasi_env_t, _memory: &wasm_memory_t) {
    panic!("wasmer_env_set_memory() is not supported");
}

/// Read captured stdout data into `buffer`.
///
/// Returns the number of bytes read, 0 if no data is available, or -1 on
/// error (e.g., stdout was not captured). This function is non-blocking;
/// for reliable results, call after the WASM module has finished execution.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_read_stdout(
    env: &mut wasi_env_t,
    buffer: *mut c_char,
    buffer_len: usize,
) -> isize {
    let inner_buffer = unsafe { slice::from_raw_parts_mut(buffer as *mut _, buffer_len) };

    if let Some(ref mut stdout_rx) = env.stdout_rx {
        read_pipe(stdout_rx, inner_buffer)
    } else {
        update_last_error("stdout is not captured; call wasi_config_capture_stdout first");
        -1
    }
}

/// Read captured stderr data into `buffer`.
///
/// Returns the number of bytes read, 0 if no data is available, or -1 on
/// error (e.g., stderr was not captured). This function is non-blocking;
/// for reliable results, call after the WASM module has finished execution.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_read_stderr(
    env: &mut wasi_env_t,
    buffer: *mut c_char,
    buffer_len: usize,
) -> isize {
    let inner_buffer = unsafe { slice::from_raw_parts_mut(buffer as *mut _, buffer_len) };

    if let Some(ref mut stderr_rx) = env.stderr_rx {
        read_pipe(stderr_rx, inner_buffer)
    } else {
        update_last_error("stderr is not captured; call wasi_config_capture_stderr first");
        -1
    }
}

fn read_pipe(pipe: &mut Pipe, buf: &mut [u8]) -> isize {
    match pipe.try_read(buf) {
        Some(n) => n as isize,
        None => 0, // No data available yet
    }
}

fn write_pipe(pipe: &mut Pipe, buf: &[u8]) -> isize {
    match std::io::Write::write(pipe, buf) {
        Ok(n) => n as isize,
        Err(err) => {
            update_last_error(format!("failed to write pipe: {err}"));
            -1
        }
    }
}

/// Write data to the captured stdin pipe.
///
/// Returns the number of bytes written, or -1 on error (e.g., stdin was not
/// captured). Data can be written before or during WASM module execution.
/// Call `wasi_env_close_stdin` after the last write so the module sees EOF.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_write_stdin(
    env: &mut wasi_env_t,
    buffer: *const c_char,
    buffer_len: usize,
) -> isize {
    let inner_buffer = unsafe { slice::from_raw_parts(buffer as *const _, buffer_len) };

    if let Some(ref mut stdin_tx) = env.stdin_tx {
        write_pipe(stdin_tx, inner_buffer)
    } else {
        update_last_error("stdin is not captured; call wasi_config_capture_stdin first");
        -1
    }
}

/// Close the stdin pipe so the module sees EOF.
///
/// Call this after writing all data via `wasi_env_write_stdin`. Without
/// closing, modules that read until EOF (e.g., jq) will hang.
/// Calling this multiple times is safe (subsequent calls are no-ops).
#[unsafe(no_mangle)]
pub extern "C" fn wasi_env_close_stdin(env: &mut wasi_env_t) {
    if let Some(ref mut stdin_tx) = env.stdin_tx {
        stdin_tx.close();
    } else {
        update_last_error("stdin is not captured; call wasi_config_capture_stdin first");
    }
}

/// Wait for all child processes spawned by WASIX `proc_fork` to finish.
///
/// WASIX modules (e.g., bash) fork child processes for pipes and subshells.
/// `wasm_func_call` returns as soon as the main process calls `proc_exit`,
/// but forked children may still be running. Call this after `wasm_func_call`
/// and before reading stdout/stderr to ensure all output has been written.
/// Returns `true` if children were waited on, `false` if there were no children.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_join_children(env: &mut wasi_env_t) -> bool {
    let _guard = env.runtime_handle.as_ref().map(|h| h.enter());

    let store = unsafe { env.store.store() };
    let data = env.inner.data(&store);
    let mut process = data.process.clone();
    // Release the store borrow before blocking; join_children doesn't need store access
    drop(store);

    block_on(async { process.join_children().await }).is_some()
}

/// Run a WASI/WASIX module's `_start` function with proper async runtime
/// and context-switching support.
///
/// This is the recommended way to run WASIX modules (e.g., bash) that use
/// `proc_fork`, pipes, or subshells. It uses the same async execution model
/// as the Rust `WasiRunner`, including context switching for asyncify-based
/// operations like `proc_fork`.
///
/// Call this after `wasi_env_initialize_instance`. Returns NULL when the
/// program runs to completion (including any `proc_exit` call, regardless
/// of exit code), or a trap for runtime-level errors. The caller should
/// delete any returned trap, and use `wasi_env_get_exit_code` to get the
/// program's exit code.
///
/// For WASIX modules that fork child processes (e.g., bash with pipes),
/// call `wasi_env_join_children` before reading stdout/stderr to ensure
/// all child output has been flushed.
///
/// After this returns, call `wasi_env_read_stdout` / `wasi_env_read_stderr`
/// to read captured output, and `wasi_env_get_exit_code` for the exit code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_start(
    wasi_env: &mut wasi_env_t,
    store: &mut wasm_store_t,
    instance: &wasm_instance_t,
) -> Option<Box<crate::wasm_c_api::trap::wasm_trap_t>> {
    // Enter the tokio runtime so WASIX child tasks can be scheduled
    let Some(handle) = wasi_env.runtime_handle.as_ref() else {
        update_last_error("wasi_start requires a tokio runtime; use wasi_env_new (not wasi_env_with_filesystem)");
        return Some(Box::new(crate::wasm_c_api::trap::wasm_trap_t::from(
            wasmer_api::RuntimeError::new("No tokio runtime handle available"),
        )));
    };
    let _guard = handle.enter();

    debug_assert!(
        wasi_env.store.ptr_eq(&store.inner),
        "wasi_start: store must be the same one passed to wasi_env_new"
    );

    // Get the _start function
    let start = match instance.inner.exports.get_function("_start") {
        Ok(func) => func.clone(),
        Err(_) => {
            update_last_error("Module has no _start function");
            return Some(Box::new(crate::wasm_c_api::trap::wasm_trap_t::from(
                wasmer_api::RuntimeError::new("Module has no _start function"),
            )));
        }
    };

    // Temporarily take ownership of the Store so we can use the async
    // execution model (ContextSwitchingEnvironment::run_main_context) which
    // requires Store by value.
    let result = unsafe {
        store.inner.with_owned_store(|owned_store| {
            wasmer_wasix::bin_factory::run_wasi_entrypoint(
                &wasi_env.inner,
                owned_store,
                start,
            )
        })
    };

    match result {
        Ok(_) => None,
        Err(e) => {
            // WASI programs exit via proc_exit, which produces a RuntimeError.
            // All proc_exit calls (zero and non-zero) are normal completions —
            // the caller should use wasi_env_get_exit_code for the exit code.
            // Only return a trap for actual runtime errors.
            match e.downcast_ref::<wasmer_wasix::WasiError>() {
                Some(wasmer_wasix::WasiError::Exit(_)) => None,
                Some(wasmer_wasix::WasiError::ThreadExit) => None,
                _ => Some(Box::new(e.into())),
            }
        }
    }
}

/// Get the exit code from the WASI environment after execution.
///
/// WASIX modules call `proc_exit` to set their exit code on the process
/// object. This function reads that exit code directly, which is more
/// reliable than parsing trap messages (since asyncify unwinding can
/// produce unrelated traps like "indirect call type mismatch").
///
/// Returns -1 if the process has not yet finished. Returns 1 as a
/// fallback if the process terminated with an error that could not be
/// mapped to an exit code (e.g., asyncify corruption).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_get_exit_code(
    wasi_env: &wasi_env_t,
) -> i32 {
    let store = unsafe { wasi_env.store.store() };
    let env = wasi_env.inner.env.as_ref(&store);
    match env.process.try_join() {
        Some(Ok(code)) => code.raw(),
        Some(Err(err)) => {
            err.as_exit_code()
                .map(|c| c.raw())
                .unwrap_or_else(|| {
                    // Asyncify may have corrupted the error; fall back to
                    // the exit code stored by proc_exit before unwinding.
                    let explicit = env.process.explicit_exit_code();
                    if explicit >= 0 { explicit } else { 1 }
                })
        }
        None => -1,
    }
}

/// Save a compiled module to the runtime's module cache under the given hash.
fn save_module_to_cache(
    wasi_env: &wasi_env_t,
    hash: ModuleHash,
    module: &wasm_module_t,
) -> bool {
    let store = unsafe { wasi_env.store.store() };
    let env = wasi_env.inner.env.as_ref(&store);
    let runtime = env.runtime.clone();
    let module_cache = runtime.module_cache();
    let engine = runtime.engine();
    // Release the store borrow before blocking
    drop(store);

    let _guard = wasi_env.runtime_handle.as_ref().map(|h| h.enter());

    match block_on(module_cache.save(hash, &engine, &module.inner)) {
        Ok(()) => true,
        Err(e) => {
            update_last_error(format!("Failed to cache module: {e}"));
            false
        }
    }
}

/// Pre-populate the module cache with a compiled module so that
/// BinFactory finds it during `proc_exec` instead of JIT-compiling.
///
/// `wasm_bytes` / `wasm_len` are the raw `.wasm` bytes of the tool.
/// `module` is an already-compiled module (e.g., from `wasm_module_deserialize`
/// with a pre-compiled static artifact).
///
/// The module is stored in the runtime's module cache under the SHA-256
/// hash of `wasm_bytes`. When WASIX bash runs `proc_exec` to invoke an
/// external command, BinFactory hashes the `.wasm` file it finds on the
/// guest filesystem, checks the cache, and — if there is a hit — uses the
/// cached module instead of calling `Module::new()` (which requires JIT).
///
/// Call this after `wasi_env_new` and before `wasi_start`, once per tool
/// that should be available to bash without JIT compilation.
///
/// Returns `true` on success, `false` on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_cache_module(
    wasi_env: &wasi_env_t,
    wasm_bytes: *const u8,
    wasm_len: usize,
    module: &wasm_module_t,
) -> bool {
    debug_assert!(!wasm_bytes.is_null());
    let bytes = unsafe { slice::from_raw_parts(wasm_bytes, wasm_len) };
    let hash = ModuleHash::new(bytes);
    save_module_to_cache(wasi_env, hash, module)
}

/// Pre-populate the WASI module cache using a pre-computed SHA-256 hash.
///
/// Like `wasi_env_cache_module`, but accepts the 32-byte hash directly
/// instead of computing it from raw `.wasm` bytes. This allows caching
/// pre-compiled static modules without needing the original `.wasm` files
/// at runtime — the hash is embedded at build time.
///
/// `hash` must point to exactly 32 bytes (SHA-256 digest).
///
/// Returns `true` on success, `false` on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_cache_module_with_hash(
    wasi_env: &wasi_env_t,
    hash: *const u8,
    module: &wasm_module_t,
) -> bool {
    debug_assert!(!hash.is_null());
    let hash_bytes: [u8; 32] = unsafe { slice::from_raw_parts(hash, 32) }
        .try_into()
        .expect("hash must be 32 bytes");
    save_module_to_cache(wasi_env, ModuleHash::from_bytes(hash_bytes), module)
}

/// The version of WASI. This is determined by the imports namespace
/// string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
#[allow(non_camel_case_types)]
pub enum wasi_version_t {
    /// An invalid version.
    INVALID_VERSION = -1,

    /// Latest version.
    ///
    /// It's a “floating” version, i.e. it's an alias to the latest
    /// version (for the moment, `Snapshot1`). Using this version is a
    /// way to ensure that modules will run only if they come with the
    /// latest WASI version (in case of security issues for instance),
    /// by just updating the runtime.
    ///
    /// Note that this version is never returned by an API. It is
    /// provided only by the user.
    LATEST = 0,

    /// `wasi_unstable`.
    SNAPSHOT0 = 1,

    /// `wasi_snapshot_preview1`.
    SNAPSHOT1 = 2,

    /// `wasix_32v1`.
    WASIX32V1 = 3,

    /// `wasix_64v1`.
    WASIX64V1 = 4,
}

impl From<WasiVersion> for wasi_version_t {
    fn from(other: WasiVersion) -> Self {
        match other {
            WasiVersion::Snapshot0 => wasi_version_t::SNAPSHOT0,
            WasiVersion::Snapshot1 => wasi_version_t::SNAPSHOT1,
            WasiVersion::Wasix32v1 => wasi_version_t::WASIX32V1,
            WasiVersion::Wasix64v1 => wasi_version_t::WASIX64V1,
            WasiVersion::Latest => wasi_version_t::LATEST,
        }
    }
}

impl TryFrom<wasi_version_t> for WasiVersion {
    type Error = &'static str;

    fn try_from(other: wasi_version_t) -> Result<Self, Self::Error> {
        Ok(match other {
            wasi_version_t::INVALID_VERSION => return Err("Invalid WASI version cannot be used"),
            wasi_version_t::SNAPSHOT0 => WasiVersion::Snapshot0,
            wasi_version_t::SNAPSHOT1 => WasiVersion::Snapshot1,
            wasi_version_t::WASIX32V1 => WasiVersion::Wasix32v1,
            wasi_version_t::WASIX64V1 => WasiVersion::Wasix64v1,
            wasi_version_t::LATEST => WasiVersion::Latest,
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_get_wasi_version(module: &wasm_module_t) -> wasi_version_t {
    get_wasi_version(&module.inner, false)
        .map(Into::into)
        .unwrap_or(wasi_version_t::INVALID_VERSION)
}

/// Non-standard function to get the imports needed for the WASI
/// implementation ordered as expected by the `wasm_module_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_get_imports(
    _store: Option<&wasm_store_t>,
    wasi_env: Option<&mut wasi_env_t>,
    module: Option<&wasm_module_t>,
    imports: &mut wasm_extern_vec_t,
) -> bool {
    unsafe { wasi_get_imports_inner(wasi_env, module, imports) }.is_some()
}

unsafe fn wasi_get_imports_inner(
    wasi_env: Option<&mut wasi_env_t>,
    module: Option<&wasm_module_t>,
    imports: &mut wasm_extern_vec_t,
) -> Option<()> {
    let wasi_env = wasi_env?;
    let store = &mut wasi_env.store;
    let module = module?;

    let mut import_object = {
        let mut store_mut = unsafe { store.store_mut() };
        c_try!(wasi_env
            .inner
            .import_object_for_all_wasi_versions(&mut store_mut, &module.inner))
    };

    let shared_memory = module.inner.imports().memories().next().map(|a| *a.ty());

    let spawn_type = match shared_memory {
        Some(ty) => wasmer_wasix::runtime::SpawnType::CreateMemoryOfType(ty),
        None => wasmer_wasix::runtime::SpawnType::CreateMemory,
    };

    let tasks = {
        let store_ref = unsafe { store.store() };
        wasi_env
            .inner
            .data(&store_ref)
            .runtime
            .task_manager()
            .clone()
    };
    let memory = {
        let mut store_mut = unsafe { store.store_mut() };
        tasks.build_memory(&mut store_mut, &spawn_type).unwrap()
    };

    if let Some(memory) = memory {
        import_object.define("env", "memory", memory.clone());
        // Store the memory for WASIX modules that import rather than export it
        wasi_env.imported_memory = Some(memory);
    }

    // Add any custom host function imports registered via wasi_env_add_host_function
    for (module_name, import_name, ext) in &wasi_env.extra_imports {
        import_object.define(module_name, import_name, ext.clone());
    }

    imports_set_buffer(store, &module.inner, import_object, imports)?;

    Some(())
}

pub(crate) fn imports_set_buffer(
    store: &StoreRef,
    module: &wasmer_api::Module,
    import_object: wasmer_api::Imports,
    imports: &mut wasm_extern_vec_t,
) -> Option<()> {
    imports.set_buffer(c_try!(
        module
            .imports()
            .map(|import_type| {
                let ext = import_object
                    .get_export(import_type.module(), import_type.name())
                    .ok_or_else(|| {
                        format!(
                            "Failed to resolve import \"{}\" \"{}\"",
                            import_type.module(),
                            import_type.name()
                        )
                    })?;

                Ok(Some(Box::new(wasm_extern_t::new(store.clone(), ext))))
            })
            .collect::<Result<Vec<_>, String>>()
    ));

    Some(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_env_initialize_instance(
    wasi_env: &mut wasi_env_t,
    store: &mut wasm_store_t,
    instance: &mut wasm_instance_t,
) -> bool {
    let mut store_mut = unsafe { store.inner.store_mut() };

    // Try the normal path first (exported memory — standard WASI modules)
    match wasi_env
        .inner
        .initialize(&mut store_mut, instance.inner.clone())
    {
        Ok(()) => return true,
        Err(wasmer_api::ExportError::Missing(_)) => {
            // No exported memory — expected for WASIX modules, fall through
        }
        Err(e) => {
            update_last_error(format!("Failed to initialize WASI instance: {e}"));
            return false;
        }
    }

    // Fall back to imported memory (WASIX modules import memory via "env"."memory")
    if let Some(memory) = wasi_env.imported_memory.clone() {
        match wasi_env
            .inner
            .initialize_with_memory(&mut store_mut, instance.inner.clone(), memory)
        {
            Ok(()) => return true,
            Err(e) => {
                update_last_error(format!("Failed to initialize WASI instance: {e}"));
                return false;
            }
        }
    }

    update_last_error("No exported or imported memory found");
    false
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasi_get_start_function(
    instance: &mut wasm_instance_t,
) -> Option<Box<wasm_func_t>> {
    let start = c_try!(instance.inner.exports.get_function("_start"));

    Some(Box::new(wasm_func_t {
        extern_: wasm_extern_t::new(instance.store.clone(), start.clone().into()),
    }))
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "windows"))]
    use inline_c::assert_c;
    #[cfg(target_os = "windows")]
    use wasmer_inline_c::assert_c;

    #[allow(
        unexpected_cfgs,
        reason = "tools like cargo-llvm-coverage pass --cfg coverage"
    )]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[test]
    fn test_wasi_get_wasi_version_snapshot0() {
        (assert_c! {
            #include "tests/wasmer.h"

            int main() {
                wasm_engine_t* engine = wasm_engine_new();
                wasm_store_t* store = wasm_store_new(engine);
                wasmer_funcenv_t* env = wasmer_funcenv_new(store, 0);

                wasm_byte_vec_t wat;
                wasmer_byte_vec_new_from_string(&wat, "(module (import \"wasi_unstable\" \"args_get\" (func (param i32 i32) (result i32))))");
                wasm_byte_vec_t wasm;
                wat2wasm(&wat, &wasm);

                wasm_module_t* module = wasm_module_new(store, &wasm);
                assert(module);

                assert(wasi_get_wasi_version(module) == SNAPSHOT0);

                wasm_module_delete(module);
                wasm_byte_vec_delete(&wasm);
                wasm_byte_vec_delete(&wat);
                wasmer_funcenv_delete(env);
                wasm_store_delete(store);
                wasm_engine_delete(engine);

                return 0;
            }
        })
        .success();
    }

    #[allow(
        unexpected_cfgs,
        reason = "tools like cargo-llvm-coverage pass --cfg coverage"
    )]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[test]
    fn test_wasi_get_wasi_version_snapshot1() {
        (assert_c! {
            #include "tests/wasmer.h"

            int main() {
                wasm_engine_t* engine = wasm_engine_new();
                wasm_store_t* store = wasm_store_new(engine);
                wasmer_funcenv_t* env = wasmer_funcenv_new(store, 0);

                wasm_byte_vec_t wat;
                wasmer_byte_vec_new_from_string(&wat, "(module (import \"wasi_snapshot_preview1\" \"args_get\" (func (param i32 i32) (result i32))))");
                wasm_byte_vec_t wasm;
                wat2wasm(&wat, &wasm);

                wasm_module_t* module = wasm_module_new(store, &wasm);
                assert(module);

                assert(wasi_get_wasi_version(module) == SNAPSHOT1);

                wasm_module_delete(module);
                wasm_byte_vec_delete(&wasm);
                wasm_byte_vec_delete(&wat);
                wasmer_funcenv_delete(env);
                wasm_store_delete(store);
                wasm_engine_delete(engine);

                return 0;
            }
        })
        .success();
    }

    #[allow(
        unexpected_cfgs,
        reason = "tools like cargo-llvm-coverage pass --cfg coverage"
    )]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[test]
    fn test_wasi_get_wasi_version_invalid() {
        (assert_c! {
            #include "tests/wasmer.h"

            int main() {
                wasm_engine_t* engine = wasm_engine_new();
                wasm_store_t* store = wasm_store_new(engine);
                wasmer_funcenv_t* env = wasmer_funcenv_new(store, 0);

                wasm_byte_vec_t wat;
                wasmer_byte_vec_new_from_string(&wat, "(module (import \"wasi_snpsht_prvw1\" \"args_get\" (func (param i32 i32) (result i32))))");
                wasm_byte_vec_t wasm;
                wat2wasm(&wat, &wasm);

                wasm_module_t* module = wasm_module_new(store, &wasm);
                assert(module);

                assert(wasi_get_wasi_version(module) == INVALID_VERSION);

                wasm_module_delete(module);
                wasm_byte_vec_delete(&wasm);
                wasm_byte_vec_delete(&wat);
                wasmer_funcenv_delete(env);
                wasm_store_delete(store);
                wasm_engine_delete(engine);

                return 0;
            }
        })
        .success();
    }

    #[allow(
        unexpected_cfgs,
        reason = "tools like cargo-llvm-coverage pass --cfg coverage"
    )]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[test]
    fn test_wasi_capture_stdout() {
        (assert_c! {
            #include "tests/wasmer.h"
            #include <string.h>

            int main() {
                // The WAT module writes "hello world" to stdout via fd_write (wasi_unstable)
                const char* wat =
                    "(module"
                    "  (import \"wasi_unstable\" \"fd_write\" (func $fd_write (param i32 i32 i32 i32) (result i32)))"
                    "  (memory 1)"
                    "  (export \"memory\" (memory 0))"
                    "  (data (i32.const 8) \"hello world\")"
                    "  (func $main (export \"_start\")"
                    "    (i32.store (i32.const 0) (i32.const 8))"
                    "    (i32.store (i32.const 4) (i32.const 11))"
                    "    (call $fd_write"
                    "      (i32.const 1)"
                    "      (i32.const 0)"
                    "      (i32.const 1)"
                    "      (i32.const 20)"
                    "    )"
                    "    drop"
                    "  )"
                    ")";

                wasm_engine_t* engine = wasm_engine_new();
                wasm_store_t* store = wasm_store_new(engine);

                // Compile module from WAT
                wasm_byte_vec_t wat_vec;
                wasmer_byte_vec_new_from_string(&wat_vec, wat);
                wasm_byte_vec_t wasm;
                wat2wasm(&wat_vec, &wasm);
                assert(wasm.size > 0);

                wasm_module_t* module = wasm_module_new(store, &wasm);
                assert(module);
                wasm_byte_vec_delete(&wasm);
                wasm_byte_vec_delete(&wat_vec);

                // Configure WASI with captured stdout
                wasi_config_t* config = wasi_config_new("test_program");
                assert(config);
                wasi_config_capture_stdout(config);

                wasi_env_t* wasi_env = wasi_env_new(store, config);
                assert(wasi_env);

                // Get WASI imports
                wasm_extern_vec_t imports;
                assert(wasi_get_imports(store, wasi_env, module, &imports));

                // Instantiate
                wasm_instance_t* instance = wasm_instance_new(store, module, &imports, NULL);
                assert(instance);
                assert(wasi_env_initialize_instance(wasi_env, store, instance));

                // Get and call _start
                wasm_func_t* start_func = wasi_get_start_function(instance);
                assert(start_func);

                wasm_val_vec_t args = WASM_EMPTY_VEC;
                wasm_val_vec_t results = WASM_EMPTY_VEC;
                wasm_trap_t* trap = wasm_func_call(start_func, &args, &results);
                assert(!trap);

                // Read captured stdout
                char buffer[128] = {0};
                intptr_t bytes_read = wasi_env_read_stdout(wasi_env, buffer, sizeof(buffer));
                assert(bytes_read == 11);
                assert(memcmp(buffer, "hello world", 11) == 0);

                // Verify that reading without capture returns -1
                wasi_config_t* config2 = wasi_config_new("test_program");
                assert(config2);
                // Do NOT call wasi_config_capture_stdout
                wasi_env_t* wasi_env2 = wasi_env_new(store, config2);
                assert(wasi_env2);
                char buffer2[16];
                assert(wasi_env_read_stdout(wasi_env2, buffer2, sizeof(buffer2)) == -1);
                assert(wasi_env_read_stderr(wasi_env2, buffer2, sizeof(buffer2)) == -1);
                wasi_env_delete(wasi_env2);

                // Cleanup
                wasm_func_delete(start_func);
                wasm_extern_vec_delete(&imports);
                wasm_instance_delete(instance);
                wasi_env_delete(wasi_env);
                wasm_module_delete(module);
                wasm_store_delete(store);
                wasm_engine_delete(engine);

                return 0;
            }
        })
        .success();
    }
}

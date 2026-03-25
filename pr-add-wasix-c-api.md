# Add WASIX support, async execution, and exit code API to C API

## Summary

Enable WASIX modules (e.g., bash) to run through the C API with proper async context-switching, multi-namespace imports, imported shared memory, and exit code retrieval.

## Problems

Three issues prevented WASIX modules from working through the C API:

### 1. Single-namespace import resolution

`wasi_get_imports` called `import_object()` which uses `get_wasi_version()` (singular) — this returns only the first WASI namespace found. For WASIX modules that import from both `wasi_snapshot_preview1` and `wasix_32v1`, only the `wasi_snapshot_preview1` imports were resolved. The `wasix_32v1` functions (`fd_dup`, `fd_pipe`, `proc_fork`, etc.) were missing, causing `wasi_get_imports` to fail.

### 2. Exported memory assumption

`wasi_env_initialize_instance` called `initialize()` which looks for exported memory. WASIX modules import shared memory via `"env"."memory"` rather than exporting it. The initialization panicked with "No exported memory found".

### 3. No async execution model

The standard C API flow (`wasi_get_start_function` + `wasm_func_call`) uses synchronous `Function::call`, which doesn't support the asyncify-based context switching that WASIX modules need for `proc_fork`, pipes, and subshells. The Rust `WasiRunner` uses `ContextSwitchingEnvironment::run_main_context` with `Function::call_async`. Without this, `proc_fork`-based bash features (subshells, pipes between builtins, here-strings) fail silently.

### 4. No exit code retrieval

After execution, there was no way for C API consumers to get the exit code. The trap returned by `wasi_start` isn't reliable because asyncify unwinding can corrupt `WasiError::Exit` into an unrelated Wasm trap (e.g., "indirect call type mismatch"), and `run_wasi_entrypoint` defaulted to `Errno::Noexec` (exit code 45) whenever the error couldn't be downcast.

## Fix

### Import resolution (`lib/c-api/src/wasm_c_api/wasi/mod.rs`)

Changed `wasi_get_imports_inner` to call `import_object_for_all_wasi_versions()` instead of `import_object()`. This resolves imports from all detected WASI/WASIX namespaces.

### Imported memory (`lib/c-api/src/wasm_c_api/wasi/mod.rs` + `lib/wasix/src/state/func_env.rs`)

- Added `initialize_with_memory()` to `WasiFunctionEnv` — like `initialize()` but accepts an explicit `Memory` parameter for modules that import rather than export memory.
- `wasi_get_imports_inner` stores the built memory in `wasi_env_t.imported_memory`.
- `wasi_env_initialize_instance` tries `initialize()` first (exported memory), then falls back to `initialize_with_memory()` using the stored imported memory.

### `wasi_start` (`lib/c-api/src/wasm_c_api/wasi/mod.rs`)

New C API function for WASIX-compatible async execution:

```c
wasm_trap_t *wasi_start(wasi_env_t *env, wasm_store_t *store, const wasm_instance_t *instance);
```

Enters the tokio runtime context, temporarily takes ownership of the `Store`, and calls `run_wasi_entrypoint` which uses `ContextSwitchingEnvironment::run_main_context` — the same async execution path as the Rust `WasiRunner`. Returns NULL for successful exits (including `proc_exit(0)`), or a trap on failure.

### `run_wasi_entrypoint` (`lib/wasix/src/bin_factory/exec.rs`)

New public function encapsulating the correct WASIX execution model: `run_main_context` → `resume_vfork` loop → `blocking_on_exit` cleanup.

### `wasi_env_get_exit_code` (`lib/c-api/src/wasm_c_api/wasi/mod.rs`)

```c
int32_t wasi_env_get_exit_code(const wasi_env_t *wasi_env);
```

Reads the exit code from the WASI process object after execution. Returns -1 if the process has not yet finished.

### Exit code preservation through asyncify (`lib/wasix`)

When `proc_exit` is called, the exit code is stored on the `WasiProcess` object via an `AtomicI32` (`explicit_exit_code`) *before* returning `Err(WasiError::Exit(code))`. `run_wasi_entrypoint` checks this as a fallback when the error can't be downcast to `WasiError::Exit`, instead of defaulting to `Errno::Noexec`.

### Supporting changes

- **`wasi_env_t.runtime_handle`**: Stores the tokio runtime handle from `wasi_env_new` so `wasi_start` can enter the runtime context.
- **`StoreRef::with_owned_store`** (`lib/c-api/src/wasm_c_api/store.rs`): Temporarily extracts the owned `Store` for `run_main_context`, which requires `Store` by value.
- **`wasi_env_join_children`**: Waits for child processes spawned by `proc_fork`.

## Results

| Command | Before | After |
|---|---|---|
| `echo hello` | works | works |
| `(echo from_subshell)` | empty | works |
| `echo hello \| while read x; do echo $x; done` | empty | works |
| `read x <<< "test"; echo $x` | empty | works |
| `jq --version` (direct) | works | works, exit code 0 |
| `jq -e 'invalid('` (direct) | works | works, exit code 3 |

## Motivation

Same as the stdio capture PR — we're embedding Wasmer via the C API in a Swift app. WASIX bash with working subshells, pipes, and exit code reporting is needed for shell command execution.

## Notes

- `import_object_for_all_wasi_versions()` already existed on `WasiFunctionEnv` but was unused by the C API.
- `run_wasi_entrypoint` reuses the existing `ContextSwitchingEnvironment` and `resume_vfork` — no new execution logic, just a public entry point with cleanup.
- `wasi_start` is additive — existing `wasm_func_call` usage is unaffected.

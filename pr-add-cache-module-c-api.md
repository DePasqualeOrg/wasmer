## Summary

Add `wasi_env_cache_module` and `wasi_env_cache_module_with_hash` to the C API so callers can pre-populate the runtime's module cache with compiled modules. This allows BinFactory to find pre-compiled modules during `proc_exec` instead of JIT-compiling them, which is required for WASIX bash pipes on iOS where JIT is unavailable.

## Problem

When WASIX bash pipes to an external command (e.g., `echo hi | cat`), the internal `BinFactory` loads the target `.wasm` file, computes its SHA-256 hash, checks the module cache, and — on a cache miss — calls `Module::new()` to JIT-compile it. On iOS without the `com.apple.security.cs.allow-jit` entitlement, `Module::new()` fails because `mprotect(PROT_EXEC)` is blocked, and the process is killed.

Even when the target tool (e.g., `cat`) has been pre-compiled to a static artifact and loaded via `wasm_module_deserialize`, BinFactory doesn't know about it. The pre-compiled module lives in the caller's scope, while BinFactory's lookup goes through the runtime's module cache. There is no C API to bridge this gap.

## Changes

### `lib/c-api/src/wasm_c_api/wasi/mod.rs`

Two new functions:

**`wasi_env_cache_module`** — caches a module by computing its hash from raw `.wasm` bytes:

```c
bool wasi_env_cache_module(const struct wasi_env_t *wasi_env,
                           const uint8_t *wasm_bytes,
                           uintptr_t wasm_len,
                           const wasm_module_t *module);
```

- Computes `ModuleHash::new(wasm_bytes)` (SHA-256), the same key BinFactory uses when looking up modules.
- Accesses the runtime's module cache via `WasiEnv::runtime.module_cache()`.
- Saves the compiled module to the cache via `block_on(cache.save(hash, &engine, &module))`.
- Returns `true` on success, `false` on error (with detail via `wasmer_last_error_message`).

**`wasi_env_cache_module_with_hash`** — caches a module using a pre-computed SHA-256 hash:

```c
bool wasi_env_cache_module_with_hash(const struct wasi_env_t *wasi_env,
                                     const uint8_t *hash,
                                     const wasm_module_t *module);
```

- Takes a 32-byte SHA-256 hash directly, using `ModuleHash::from_bytes()`.
- Same cache insertion logic as `wasi_env_cache_module`.
- Allows callers to embed the hash at build time (e.g., computed during `wasmer create-obj`) and skip bundling the raw `.wasm` files at runtime.

Both functions take shared references (`&wasi_env_t`) since they never mutate the environment — they only read the store, runtime, and tokio handle. This matches the convention of `wasi_env_get_exit_code`.

### Intended usage

Call after `wasi_env_new` and before `wasi_start`, once per tool that should be available to bash without JIT.

With raw `.wasm` bytes available:

```c
wasm_module_t *module = wasm_module_deserialize(store, &static_metadata);
wasi_env_cache_module(wasi_env, raw_wasm_bytes, raw_wasm_len, module);
wasm_module_delete(module);
```

With a pre-computed hash (no `.wasm` files needed at runtime):

```c
wasm_module_t *module = wasm_module_deserialize(store, &static_metadata);
wasi_env_cache_module_with_hash(wasi_env, build_time_sha256, module);
wasm_module_delete(module);
```

When bash later runs `proc_exec("cat")`, BinFactory reads `/tools/cat` from the guest filesystem, hashes the bytes, checks the cache, and gets a hit — no JIT compilation needed.

## Testing

- All 17 Swift tests pass on macOS (15 pass, 2 known failures due to unpatched bash binary — not related to this change).
- Bash pipe (`echo hi | cat`) confirmed working on iOS device without JIT entitlement — the previous crash is resolved.

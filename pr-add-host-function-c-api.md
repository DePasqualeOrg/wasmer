## Summary

Add `wasi_env_add_host_function` to the C API, allowing custom host function imports to be registered alongside standard WASI imports. This enables instantiation of Wasm modules that import non-WASI functions (e.g., a shell interpreter that calls back into the host to dispatch external commands).

## Problem

`wasi_get_imports` resolves a module's imports by looking them up in the WASI import object. If a module has any non-WASI import (e.g., `env.exec_command`), import resolution fails with "Failed to resolve import", and the module cannot be instantiated.

There is no C API mechanism to add custom imports alongside WASI imports. The only workaround is to manually build the entire import vector outside of `wasi_get_imports`, re-implementing its WASI resolution and memory setup logic.

## Changes

### `lib/c-api/src/wasm_c_api/wasi/mod.rs`

**New field on `wasi_env_t`:**

```rust
extra_imports: Vec<(String, String, wasmer_api::Extern)>
```

Stores custom imports registered before import resolution.

**New function `wasi_env_add_host_function`:**

```c
bool wasi_env_add_host_function(
    struct wasi_env_t *wasi_env,
    const char *module_name,
    const char *import_name,
    const wasm_func_t *func
);
```

Registers a host function to be included in the import object when `wasi_get_imports` is called. The function is cloned internally. Multiple calls can register multiple imports.

**Modified `wasi_get_imports_inner`:**

After building the WASI import object and setting up memory, any registered extra imports are added to the import object before calling `imports_set_buffer`. This is a three-line addition:

```rust
for (module_name, import_name, ext) in &wasi_env.extra_imports {
    import_object.define(module_name, import_name, ext.clone());
}
```

## Use case

A shell interpreter (mvdan/sh compiled to WASI) imports `env.exec_command` as a host function. When the shell needs to run an external command (cat, grep, jq), it calls this import. The host provides the implementation, which runs the tool's Wasm module and returns the result. Without this API, the shell module cannot be instantiated through the standard `wasi_get_imports` path.

## Testing

Validated by compiling swift-wasm-runtime with the new API: `wasi_env_add_host_function` successfully registers the host function, and `wasi_get_imports` resolves both WASI and custom imports for a Go-based shell module that imports `env.exec_command(i32, i32, i32, i32) -> i64`.

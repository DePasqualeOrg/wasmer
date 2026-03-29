## Summary

Add C API support for running `.webc` packages with full WASIX dynamic linking. This enables tools like Python (which bundle shared libraries and use WASIX dynamic linking) to be loaded and executed through the C API.

## Problem

The existing `wasi_env_with_filesystem` C API only supports `.webc` v1 packages (using `StaticFileSystem` and `webc::v1::WebC`). Modern packages on the Wasmer Registry use v3, and dynamically-linked WASIX modules (like Python) require the full `Linker` for instantiation — which wasn't exposed through the C API.

## Changes

### New C API functions

- **`wasi_filesystem_get_atom(fs, atom_name, out_bytes)`**: Extract the raw wasm module bytes for a named atom from a `.webc` package. Uses `wasmer_package::Container::atoms()` which handles all `.webc` versions (v1, v2, v3).

- **`wasi_env_instantiate_webc(config, store, module, fs, wasi_env_out)`**: Create a WASI environment and instantiate a module from a `.webc` package in one call. Uses `WasiEnv::instantiate()` internally, which routes dynamically-linked modules through the full `Linker` (resolving `env.__memory_base`, C++ TLS symbols, side modules like `libsqlite3.so`). Returns both the instance and the WASI environment with a tokio runtime handle, so `wasi_start` works for WASIX execution.

- **`imports_set_buffer_with_stubs`** (internal): Generates trap stubs for unresolved function imports in the `wasi_env_with_filesystem` path. Needed for WASIX modules that import C++ runtime symbols not covered by standard WASI/WASIX exports.

### Rewritten `prepare_webc_env`

Replaced the v1-only `StaticFileSystem` + `WebC::parse_volumes_from_fileblock` with version-agnostic `WebcVolumeFileSystem` + `UnionFileSystem`. Volumes are mounted according to the manifest's `[fs]` mappings (e.g., `/lib` → volume "root" path "lib"), matching how `wasmer run` handles v3 packages.

Extracted shared `mount_webc_filesystem` helper to avoid duplication between `prepare_webc_env` and `wasi_env_instantiate_webc_inner`.

### Visibility changes

- `virtual_fs::webc_volume_fs` module made `pub` (was private, needed by C API)
- `WasiEnv::instantiate()` made `pub` (was `pub(crate)`, needed by C API for WASIX linker access)

### Dependency additions

- `wasmer-package` added to `wasmer-c-api` (for `.webc` parsing)
- `webc-fs` feature added to `virtual-fs` dependency (for `WebcVolumeFileSystem`)
- `webc_runner` feature updated to include `wasmer-package`

## Testing

Validated by running CPython from the `python/python` `.webc` package (v3, 43 MB, WASIX dynamic linking) through a Swift host application. 51 tests passing including Python hello world, JSON module import, stdin processing, exit codes, and shell dispatch via host function bridge.

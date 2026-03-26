# Fix `on_exit` crash when WASI instance was never initialized

## Summary

`WasiFunctionEnv::on_exit` calls `self.data(store).inner()` unconditionally, which panics if `inner` was never set via `wasi_env_initialize_instance`. This happens when `wasm_instance_new` fails (e.g., memory allocation error) and the caller cleans up by deleting the WASI environment — the `wasi_env_delete` path calls `on_exit`, which panics because the instance handles were never initialized.

The fix uses `try_inner()` instead of `inner()` to gracefully handle the uninitialized case.

## Reproduction

Call `wasi_env_new` → `wasi_get_imports` → `wasm_instance_new` (which fails) → `wasi_env_delete`. The delete triggers `on_exit` → `inner()` → panic.

On iOS, this is reliably triggered because the default memory tunables request 6 GiB of virtual address space, which exceeds iOS's limits. `wasm_instance_new` fails with `ENOMEM`, and the subsequent cleanup panics.

## Changes

- `lib/wasix/src/state/func_env.rs`: `on_exit` now calls `try_inner()` and skips the linker shutdown if the instance was never initialized.

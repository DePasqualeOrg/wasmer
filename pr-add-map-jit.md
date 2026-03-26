## Summary

Add `MAP_JIT` support on Apple aarch64 (macOS and iOS) so JIT-compiled code can execute under macOS Hardened Runtime and on iOS with the JIT entitlement. Without this, `mprotect` to `PROT_EXEC` fails on Hardened Runtime and iOS because the pages lack a code signature.

## Problem

Wasmer's code memory allocation uses `mmap` with `PROT_READ | PROT_WRITE`, then `region::protect` (which calls `mprotect`) to switch to `PROT_READ | PROT_EXEC` after writing compiled code. This works on macOS without Hardened Runtime, but fails in two environments:

- **macOS with Hardened Runtime**: Required for App Store and notarized macOS apps. Blocks `mprotect` to `PROT_EXEC` on unsigned pages.
- **iOS**: Always enforces code signing on executable pages. `mprotect` to `PROT_EXEC` is unconditionally rejected.

Apple provides `MAP_JIT` + `pthread_jit_write_protect_np` as the supported mechanism for JIT on these platforms. Pages allocated with `MAP_JIT` can be toggled per-thread between writable (for code generation) and executable (for running), enforcing W^X at all times. This is the same mechanism used by JavaScriptCore.

## Changes

### New module: `lib/vm/src/apple_jit.rs`

Centralizes the MAP_JIT runtime support behind three functions:

- `is_supported()`: Determines whether MAP_JIT is both available and needed (result cached in `OnceLock`). First checks that `pthread_jit_write_protect_np` exists via `dlsym` (avoiding a hard link dependency, which matters for iOS where the default Rust deployment target predates the API). Then probes whether `mprotect(PROT_READ | PROT_EXEC)` works on a test page. If mprotect works (macOS development builds without Hardened Runtime), MAP_JIT is not needed and the traditional mprotect path is used. If mprotect fails (iOS, macOS Hardened Runtime with `allow-jit`), MAP_JIT is needed. This distinction matters because `pthread_jit_write_protect_np` exists on all recent macOS versions but MAP_JIT pages only function correctly with the JIT entitlement — on development macOS, toggling MAP_JIT pages to executable via `pthread_jit_write_protect_np(1)` and then executing from them crashes with SIGBUS.
- `enable_write()`: Toggles the current thread's MAP_JIT pages to writable. Called before writing compiled code.
- `enable_execute(ptr, len)`: Toggles to executable and flushes the instruction cache via `sys_icache_invalidate`. Called after code is written.

All three are no-ops when `is_supported()` returns false.

### `lib/vm/src/mmap.rs`

- Refactored `accessible_reserved` into a public wrapper and private `accessible_reserved_inner` with a `for_code: bool` parameter.
- `with_at_least` (used only by `CodeMemory` for code pages) passes `for_code: true`. `accessible_reserved` (used by Wasm linear memory) passes `for_code: false`.
- When `for_code` is true and `apple_jit::is_supported()` returns true, `MAP_JIT` is added to the mmap flags. This ensures `MAP_JIT` is never applied to Wasm linear memory.
- `with_at_least` has separate `#[cfg]` implementations for Windows (delegates to `accessible_reserved`, no `for_code`) and non-Windows (delegates to `accessible_reserved_inner` with `for_code: true`).

### `lib/compiler/src/engine/code_memory.rs`

- In `allocate`: calls `apple_jit::enable_write()` before copying compiled code into the mmap pages.
- In `publish`: when `apple_jit::is_supported()` is true, calls `apple_jit::enable_execute()` instead of `region::protect`. When not supported, falls back to the original `region::protect` path. The `is_supported()` check is consistent with the `MAP_JIT` allocation decision in `mmap.rs`, so the two mechanisms are always in agreement.

### `.cargo/config.toml`

Sets the iOS deployment target to 14.0 (when `pthread_jit_write_protect_np` was introduced). Without this, the Rust target `aarch64-apple-ios` defaults to iOS 10.0, causing linker errors for the cdylib output.

### Scope

All changes are gated behind `#[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]`. Non-Apple and x86_64 Apple platforms are completely unaffected.

## Testing

Tested on macOS aarch64 (development build, no Hardened Runtime). The runtime probe detects that `mprotect(PROT_EXEC)` works and uses the traditional path — `is_supported()` returns false. All 16 tests pass (both JIT compilation from `.wasm` files and pre-compiled static artifact loading).

iOS device testing is still needed to confirm the MAP_JIT path works with the `com.apple.security.cs.allow-jit` entitlement (where `mprotect(PROT_EXEC)` fails and `is_supported()` returns true).

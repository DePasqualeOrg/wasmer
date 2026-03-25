## Summary

Fix C API stdio capture (`wasi_config_capture_stdout` / `wasi_env_read_stdout` and stderr equivalents) which was completely broken, and add the missing stdin capture support.

## Problem

The C API's `wasi_env_new` set up captured stdout/stderr like this:

```rust
config.builder.set_stdout(Box::new(Pipe::channel().0));
```

`Pipe::channel()` returns two cross-connected pipe ends `(end1, end2)`. Taking `.0` and dropping `.1` severed the channel in both directions:

1. **Writes failed**: the module wrote to `end1.send`, but the receiver (inside dropped `end2`) was gone – writes returned `BrokenPipe`.
2. **Reads returned nothing**: `wasi_env_read_stdout` read from `end1.recv`, which received from the sender in dropped `end2` – no data ever arrived.

Stdin capture was left unimplemented (`// TODO: impl capturer for stdin`).

## Fix

- **`wasi_env_t`** gains three fields: `stdout_rx`, `stderr_rx`, `stdin_tx` – the host-side pipe ends.
- **`wasi_env_new`** and **`prepare_webc_env`** keep both ends of `Pipe::channel()`: one goes to the WASI builder, the other is stored in `wasi_env_t`.
- **`wasi_env_read_stdout` / `wasi_env_read_stderr`** read from the stored pipe end using non-blocking `try_read` (the async `read` would deadlock since the write end stays alive in the WASI env's fd table after the module exits).
- **`wasi_config_capture_stdin`** is uncommented and functional.
- **`wasi_env_write_stdin`** is implemented (the signature was already declared in `wasmer.h`).
- **`wasi_env_close_stdin`** is added so the module sees EOF after the host finishes writing. Without this, modules that read until EOF (e.g., jq) hang indefinitely.
- Calling `wasi_env_read_stdout` without prior `wasi_config_capture_stdout` now returns -1 with a clear error message instead of silently failing.

## Motivation

We're embedding Wasmer via the C API in a Swift app to run WASI/WASIX tools on iOS/macOS. Programmatic stdio capture is required to pass data into and read results from modules without using the host process's actual stdio.

## Changes

All changes are in `lib/c-api/src/wasm_c_api/wasi/mod.rs`.

## Testing

Verified with a standalone C test linked against `libwasmer.dylib`:

- **stdout capture**: module writes "hello world" via `fd_write` to fd 1 → host reads it back via `wasi_env_read_stdout`
- **stderr capture**: module writes "error msg" via `fd_write` to fd 2 → host reads it back via `wasi_env_read_stderr`
- **stdin capture**: host writes "hello from stdin" via `wasi_env_write_stdin` → module reads from fd 0 and echoes to stdout → host reads it back
- **empty read**: reading captured stdout before any output returns 0

An `assert_c!` inline test (`test_wasi_capture_stdout`) is also included in the source, following the existing test pattern.

## Notes

- The C header (`wasmer.h`) already declared `wasi_config_capture_stdin` and `wasi_env_write_stdin` – no header changes needed.
- The `Pipe` implementation was reworked in Feb 2025 (commit `f1ba57784df` – "Rework WASIX pipes to be simplex") but the C API was never updated to match. The Rust API's `WasiRunner.with_stdout()` and tests in `lib/wasix/tests/stdio.rs` already used the correct two-ended pattern.

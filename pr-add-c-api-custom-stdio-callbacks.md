## Summary

Add Wasmtime-style custom stdout/stderr callbacks to Wasmer's C API WASI config.

This introduces:

- `wasi_config_set_stdout_custom`
- `wasi_config_set_stderr_custom`
- `wasi_config_write_callback_t`

The existing `wasi_config_capture_stdout` and `wasi_config_capture_stderr`
behavior remains unchanged.

## Motivation

Embedders sometimes need direct control over captured stdio so they can:

- bound memory usage
- stream output incrementally
- stop accepting output once a consumer is done

Wasmtime already supports this style of host-controlled stdio through callback
configuration. This change brings the same kind of control to Wasmer's C API
without changing default behavior for existing users.

## Design

The new API matches the Wasmtime shape:

- the embedder provides a callback
- the callback receives each stdout/stderr write
- the callback returns the number of bytes accepted
- returning `0` signals that no more output should be accepted
- negative return values are treated as OS error codes

An optional finalizer can be provided for the callback userdata.

Internally, the C API routes stdout/stderr through a custom virtual file that
invokes the embedder callback on each write. A write refusal is converted into
`BrokenPipe`, which flows through the existing WASIX `fd_write` path as
`Errno::Pipe`.

## Why this shape

This keeps the policy with the consumer instead of hardcoding it into Wasmer.
For example, an embedder can now implement:

- a fixed-size buffer
- line-by-line streaming
- log forwarding
- early termination once a downstream consumer closes

## Testing

- `cargo build -p wasmer-c-api`
- `cargo test -p wasmer-c-api test_callback_output_file -- --nocapture`

I also confirmed the generated `lib/c-api/wasmer.h` exposes:

- `wasi_config_write_callback_t`
- `wasi_config_set_stdout_custom`
- `wasi_config_set_stderr_custom`

## Notes

- This is opt-in and does not alter existing capture or inherit behavior.
- The existing inline C WASI capture tests were not a reliable local signal in
  this environment, so the added verification focuses on the callback path
  directly.

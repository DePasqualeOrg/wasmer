## Summary

Wire the C API's `compiler-headless` feature through to `wasmer-api/headless` so the headless sys engine actually compiles.

## Problem

`wasmer-c-api` exposes a `compiler-headless` feature and its sys engine config already uses `EngineBuilder::headless()` when that feature is enabled.

However, `lib/c-api/Cargo.toml` only enabled `wasmer-api/compiler` for that path. In `wasmer-api`, `headless` is a separate backend feature, and `sys` requires at least one backend among `singlepass`, `cranelift`, `llvm`, or `headless`.

That left `wasmer-c-api --no-default-features --features compiler-headless` in an invalid state:

- `wasmer-api/sys` was enabled
- no compiler backend feature was enabled
- the build failed with Wasmer's own compile-time feature checks

## Fix

Add `wasmer-api/headless` to the `compiler-headless` feature in `lib/c-api/Cargo.toml`.

This makes the declared C API feature match the implementation that already calls `EngineBuilder::headless()`.

## Testing

Validated in a clean temporary checkout with:

```sh
cargo check -p wasmer-c-api --no-default-features --features compiler-headless
```

Before the change, the build failed in `wasmer-api` with:

- `wasmer requires enabling at least one backend feature`
- `the sys feature requires enabling at least one compiler backend`

After the change, the build progressed past that feature-resolution failure and reached a separate existing `dead_code` error in `wasmer-c-api`, confirming the headless wiring issue itself was fixed.

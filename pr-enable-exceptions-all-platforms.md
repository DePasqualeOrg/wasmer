## Summary

Enable the Wasm exception handling proposal on all platforms, not just Linux.

## Problem

Newer packages in the Wasmer Registry (grep 3.12, sed 4.9) are compiled with the Wasm exception handling proposal. When loaded on macOS or iOS, they fail with:

```
No backends support the required features: exceptions proposal not enabled
```

The feature was gated on Linux in `supported_features_for_target`:

```rust
if target.triple().operating_system == OperatingSystem::Linux {
    feats.exceptions(true);
}
```

## Changes

### `lib/compiler-cranelift/src/config.rs`

Remove the Linux gate, enable `feats.exceptions(true)` unconditionally. The Cranelift backend supports exception handling on all platforms — the gate was likely a conservative default rather than a technical limitation.

## Testing

Validated on macOS (aarch64): grep 3.12 and sed 4.9 from the Wasmer Registry now load and execute correctly, both standalone and through WASIX bash pipes.

# Reduce memory tunables for Apple embedded platforms

## Summary

The default 64-bit tunables reserve 4 GiB (static bound) + 2 GiB (guard) = 6 GiB virtual address space per Wasm linear memory. Apple embedded platforms (iOS, watchOS, tvOS, visionOS) typically have 2 – 4 GiB total virtual address space, so `mmap` fails with `ENOMEM` during instantiation.

This adds a branch in `BaseTunables::for_target` for Apple embedded platforms that uses a 1 GiB static bound (`0x4000` pages) and 64 KiB guard (`0x1_0000`), matching the 32-bit defaults. Modules without a declared maximum (most WASI modules) use Dynamic memory style; modules with a maximum <= 1 GiB use Static style with bounds-check elimination.

## Platforms affected

`OperatingSystem::IOS`, `WatchOS`, `TvOS`, `VisionOS`, `XROS` -- all Apple embedded platforms with constrained virtual memory. macOS is not affected (retains the 4 GiB + 2 GiB defaults).

## Why a runtime check instead of `#[cfg]`

The check uses `matches!` against the target triple's OS rather than `#[cfg(target_os = "ios")]`. This is intentional -- `wasmer create-obj --target aarch64-apple-ios` runs on macOS, so the compile-time `#[cfg]` would not apply. The runtime triple check ensures the reduced tunables are used both at runtime on the device and during cross-compilation.

## Changes

- `lib/compiler/src/engine/tunables.rs`: `BaseTunables::for_target` checks the target triple's OS for Apple embedded platforms and uses reduced memory bounds.

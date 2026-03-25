## Summary

Fix static object compilation and linking for Apple platforms (macOS and iOS). Re-enables the `create-obj` command, fixes Mach-O ARM64 relocation encoding that prevented linking, and switches iOS builds to `native-tls` so the full feature set (including networking) compiles for iOS.

## Problem

Three issues prevented using Wasmer's static artifact pipeline (`create-obj` → link → `deserialize_object`) on Apple platforms:

1. **`create-obj` was disabled.** The command was commented out in the CLI registration (`lib/cli/src/commands/mod.rs`) alongside `create-exe` due to unresolved issues. But the issues were in `create-exe` (the linker step), not `create-obj` (the compilation step). Object file generation works fine.

2. **Mach-O ARM64 relocations were encoded incorrectly.** All Mach-O ARM64 relocation entries in `lib/compiler/src/object/module.rs` used wrong `r_length` values. The `object` crate's `RelocationFlags::MachO { r_length }` field expects the raw Mach-O log2 encoding (0 = 1 byte, 1 = 2 bytes, 2 = 4 bytes, 3 = 8 bytes), but the code passed bit sizes (32, 64). Since `r_length` is a `u8`, the value 32 (`0b00100000`) was silently truncated, producing `r_length=0` (1 byte) — completely wrong for 4-byte ARM64 instructions. Additionally, `ARM64_RELOC_UNSIGNED` had `r_pcrel: true` (should be `false`), and several `PAGEOFF12`/`GOT_LOAD_PAGEOFF12` variants had incorrect `r_pcrel` values. The macOS linker rejected every `.o` file produced by `create-obj`.

3. **iOS builds failed due to `aws-lc-sys`.** The `sys` feature enables `host-reqwest` → `reqwest` (with `rustls`) → `aws-lc-rs` → `aws-lc-sys`. The `aws-lc-sys` C library references `___chkstk_darwin`, which doesn't exist on iOS. This blocked building `libwasmer` for iOS entirely.

## Changes

### Re-enable `create-obj` (`lib/cli/src/commands/mod.rs`)

Uncomment the `CreateObj` enum variant and its match arm in `WasmerCmd`. The command was disabled alongside `create-exe`, but only `create-exe` has unresolved issues.

### Fix Mach-O ARM64 relocations (`lib/compiler/src/object/module.rs`)

Correct `r_length` and `r_pcrel` values for all Mach-O ARM64 relocation types:

| Relocation | `r_length` before → after | `r_pcrel` before → after |
|---|---|---|
| `Arm64Call` / `BRANCH26` | 32 → 2 | (unchanged) |
| `UNSIGNED` | 32 → 3 | true → false |
| `SUBTRACTOR` | 64 → 3 | (unchanged) |
| `PAGE21` | 32 → 2 | (unchanged) |
| `PAGEOFF12` | 32 → 2 | (unchanged) |
| `GOT_LOAD_PAGE21` | 32 → 2 | (unchanged) |
| `GOT_LOAD_PAGEOFF12` | 32 → 2 | true → false |
| `POINTER_TO_GOT` | 32 → 2 | (unchanged) |
| `TLVP_LOAD_PAGE21` | 32 → 2 | (unchanged) |
| `TLVP_LOAD_PAGEOFF12` | 32 → 2 | true → false |
| `ADDEND` | 32 → 2 | (unchanged) |

### Switch iOS to `native-tls` (`lib/wasix/Cargo.toml`)

Add `target_os = "ios"` to the existing platform condition that selects the `native-tls` reqwest feature instead of `rustls`. This follows the existing pattern for `riscv64` and `loongarch64`. On iOS, `native-tls` uses Apple's Security.framework, preserving full HTTP/HTTPS networking with no features lost.

## Testing

After these fixes, the full static artifact pipeline works on macOS:

```
# Compile .wasm to .o
wasmer create-obj jq.wasm -o jq.o --target aarch64-apple-darwin

# Link into a test binary with libwasmer.a
cc test.c jq.o libwasmer.a -o test

# Run — jq executes via deserialize_object, no JIT needed
./test
# Output: jq-1.6
```

Both WASI (jq) and WASIX (bash) modules work through the static path, including async context switching for WASIX's `proc_fork` and pipes.

iOS cross-compilation also succeeds:
```
cargo build --release -p wasmer-c-api --target aarch64-apple-ios \
    --no-default-features --features wat,compiler,cranelift,wasi,static-artifact-load
```

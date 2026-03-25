# Fix proc_exec for mapped directories in C API

## Summary

Fix `proc_exec` in the C API so that WASIX bash can run external WASM binaries (e.g., `jq`, `cat`) from mapped directories.

## Problem

When using the C API with `wasi_config_mapdir` to map a host directory (e.g., `/tools` → host path), bash builtins and subshells work, but running external commands fails with `Errno::Noexec` (exit code 45):

```
echo hello                          →  works (builtin)
(echo hello)                        →  works (proc_fork)
echo hello | while read x; do …    →  works (pipe between builtins)
echo '{"a":1}' | jq '.a'           →  empty (jq never runs)
jq --version                        →  exit code 45
```

### Root cause

Two different path resolution mechanisms exist in the WASI filesystem:

1. **Inode tree** (`get_inode_at_path`): Bash's path lookup traverses preopened directory entries. For `/tools/jq`, it finds the `/tools` preopened dir (which stores the host path), then calls `root_fs.symlink_metadata(host_path/jq)`. This works because `root_fs` is a host filesystem.

2. **Direct filesystem access** (`BinFactory::get_executable`): When `proc_exec` spawns an external binary, `BinFactory` passes the guest path `/tools/jq` directly to `root_fs.open("/tools/jq")`. Since there's no `/tools/jq` on the real host filesystem, the open fails with `EntryNotFound`.

`wasi_config_mapdir` only created preopened directory entries in the inode tree. It did not set up filesystem mounts, so guest alias paths were invisible to `root_fs` and therefore to `BinFactory`.

The Rust CLI avoids this by using a `TmpFileSystem` with explicit `mount()` calls for each mapped directory, making guest paths resolvable at the filesystem level.

## Fix

### Filesystem setup in `wasi_env_new` (`lib/c-api/src/wasm_c_api/wasi/mod.rs`)

Changed the C API to set up the filesystem the same way the CLI does:

1. **Track mapped directories** in `wasi_config_t` when `wasi_config_mapdir` is called, instead of immediately calling `add_map_dir`.

2. **In `wasi_env_new`**, if there are mapped directories:
   - Normalize guest paths (ensure leading `/`) once up front
   - Create a `TmpFileSystem` via `RootFileSystemBuilder::build_ext`, excluding mapped guest paths from default dirs to avoid `AlreadyExists` collisions (e.g., if mapping `/bin` or `/tmp`)
   - Create intermediate directories for nested guest paths (e.g., `/usr/local/bin`)
   - Mount the host filesystem at each guest alias path: `root_fs.mount("/tools", &host_fs, host_dir)`
   - Register preopened dirs via `add_map_dir` using the guest path as the directory path (since the mount makes it resolvable in `root_fs`)
   - Set the `TmpFileSystem` as the builder's sandbox filesystem and preopen `/` for root directory access

3. **If no mapped directories**, fall back to `default_fs_backing()` (host filesystem) as before.

This makes both resolution mechanisms work:
- **Inode tree**: Preopened dir's `Kind::Dir { path: "/tools" }` + lazy loading calls `root_fs.symlink_metadata("/tools/jq")` → mount redirects to `host_fs` at `host_dir/jq`
- **BinFactory**: `root_fs.open("/tools/jq")` → mount redirects to `host_fs.open(host_dir/jq)`

## Results

All bash features now work through the C API, including running external WASM binaries:

| Command | Before | After |
|---|---|---|
| `echo hello` | works | works |
| `(echo from_subshell)` | works | works |
| `echo hello \| while read x; do echo $x; done` | works | works |
| `jq --version` | exit 45 | works |
| `echo '{"a":1}' \| jq '.a'` | empty | works |
| `cat /mnt/data.json \| jq '.key'` | empty | works |

## Motivation

Same as the prior PRs — we're embedding Wasmer via the C API in a Swift app. Running external WASM tools from within bash (piping data through jq, using cat on mounted files, etc.) is needed for shell command execution.

## Notes

- The fix is additive and only affects the `wasi_config_mapdir` + `wasi_env_new` code path. Direct tool execution via `wasm_func_call` or `wasi_start` is unaffected.
- `wasi_config_preopen_dir` and `wasi_config_mapdir` cannot be used together — mapped directories use a sandboxed `TmpFileSystem`, and preopened host paths won't resolve against it. `wasi_env_new` returns an error with a clear message if both are used. `wasi_config_preopen_dir` alone continues to work as before (no sandbox, host filesystem).
- The `TmpFileSystem` + mount approach is exactly what the CLI's `wasi.rs` does for `--mapdir` arguments. No new filesystem abstractions were introduced.

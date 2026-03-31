## Summary

Fix `path_remove_directory` failing with `Errno::Noent` when the target directory hasn't been traversed in the current WASI instance.

## Motivation

The WASIX kernel maintains an in-memory inode table with lazily populated directory entries. When a WASI module calls `path_remove_directory`, the implementation looked up the child entry directly in the parent's `entries` HashMap. If the parent directory had never been traversed in this WASI instance, `entries` was empty and the call failed with `Noent` — even though the directory existed on the host filesystem.

This surfaced in an embedder that dispatches shell commands to separate WASI tool instances: `mktemp -d` created a directory in one instance, and `rmdir` in a fresh instance couldn't find it.

## Fix

Add a `get_inode_at_path` call before accessing the parent's entries, which triggers the lazy-load path in `get_inode_at_path_inner`. This is the same pattern already used by `path_unlink_file`, which works correctly for the same reason.

## Changes

- **`path_remove_directory.rs`**: Call `get_inode_at_path` to ensure the child inode is loaded into the parent's entries cache before the removal logic runs.

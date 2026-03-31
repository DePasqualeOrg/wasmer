## Summary

Cache the directory entry snapshot across paginated `fd_readdir` calls so that cookie-based iteration returns a consistent view of the directory.

## Motivation

WASI's `fd_readdir` uses cookie-based pagination: the caller provides a buffer and a cookie offset, receives as many entries as fit, then calls again with the next cookie to continue. The old implementation re-scanned the host filesystem and re-sorted entries on every call. This meant that if the directory contents changed between calls — or if the host returned entries in a different order — the cookie offsets would be stale, and entries could be skipped or duplicated.

The upstream code acknowledged this with a TODO: "we need to support multiple calls."

## Design

A new `readdir_state` field on the `Fd` struct holds an `Arc<Mutex<Option<Arc<Vec<...>>>>>` that caches the sorted entry snapshot for the current readdir sequence. The cache is populated (or refreshed) when `cookie == 0` — the WASI convention for "start from the beginning" — and reused for all subsequent calls in the same sequence.

The entry-collection logic is extracted into a standalone `collect_directory_entries` function with no change to the collection or sorting behavior itself.

## Changes

- **`fd.rs`**: Add `readdir_state` field to `Fd`.
- **`fd_readdir.rs`**: Extract `collect_directory_entries`; cache its result in `readdir_state`.
- **`fd_list.rs`**, **`mod.rs`**, **`fd_renumber.rs`**, **`proc_spawn2.rs`**: Include `readdir_state` at every `Fd` construction and clone site.

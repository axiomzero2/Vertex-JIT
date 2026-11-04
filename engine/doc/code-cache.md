# Executable Code Cache

## Abstract

`engine/src/cold/code_cache.rs` manages the executable memory that compiled
tiers publish into. Regions are mapped read-write, filled, and sealed to
read-execute before execution; execution faults on writes and writes fault
on execution. The cache is budgeted: allocation beyond the budget fails with
a typed error instead of growing silently.

## Formal Definitions

A region is a contiguous byte range inside one `mmap`ed block with page
granularity. The cache exposes `allocate(len)`, `seal(base)`, `write`-during-
allocation and `release(base)`. The target contract (`platform.rs`) supplies
page size and cache-line size.

## Algorithm

Allocation walks a free list in address order and splits the first fit that
satisfies alignment. Sealing flips the region's pages to RX with
`mprotect`. Release returns the range to the free list after re-protecting
to RW. The engine releases on deopt-driven invalidation so replacement code
can be published.

## Invariants

1. No byte is executable before `seal`; no byte is writable after it.
2. Allocation is deterministic for a given sequence of requests: address
   order, first fit, no randomness.
3. The cache never exceeds its budget; overflow is the `CodeCacheFull`
   error, which the engine surfaces instead of evicting.
4. A released region is unreachable: entries are dropped from the compiled
   table before release (the engine holds the ordering).

## Integration Points

The engine publishes T0 bytes and lowered IR bytes through the same cache.
The trampoline only calls addresses the cache handed out; the deopt path
validates `code_base` against the cache's metadata before reconstructing
frames (doc/deopt.md).

## Known Limitations

1. There is no compaction; long-running processes with heavy invalidation
   fragment the cache until the budget refuses.
2. W^X toggling is per allocation, not per batch; heavy publication costs
   one `mprotect` per seal (measured in benches/EVIDENCE.md).

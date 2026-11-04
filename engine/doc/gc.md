# Garbage Collection

## Abstract

`engine/src/cold/gc.rs` and `engine/src/hot/heap.rs` implement the managed
heap: a young generation with bump allocation, an old generation, humongous
object handling, and the barriers and safepoints compiled code uses. The
design is single-mutator: collections run at allocation slow paths and at
engine-driven points, never concurrently with generated code.

## Formal Definitions

An object is a header (tag, size, mark/state word) followed by payload words.
References are boxed words with the string/array tag convention of
doc/value.md. The young space is a bump arena; survivors are copied to the
old space; humongous objects (above the large threshold) are allocated
directly in their own region.

The write barrier records old-to-young stores into a card table; the read
barrier is only needed for concurrent old-generation processing, which the
single-mutator design does not exercise yet.

## Algorithm

Allocation fast path: bump the young cursor, compare against the limit, and
return; the slow path triggers `GcNeeded`, which the interpreter observes
and answers with `collect_young`. A young collection copies reachable
objects to the old space (promotion) or within the young space, updates the
mutator's roots (frames, engine tables), and resets the cursor. The engine
collects between budget windows, never inside generated code.

## Invariants

1. Generated code allocates only through `jit_alloc`; a zero return is a
   contract the caller's template turns into a trap (doc/codegen.md).
2. Barriers fire on every reference store generated code emits; redundant
   barriers are the optimizer's business, not the collector's.
3. A collection never moves an object the mutator can observe as pinned by
   a machine frame: frame roots are scanned from the interpreter's frame
   list and the trampoline's saved state.
4. The heap is budgeted; exhaustion is the `OutOfMemory` error, loud per
   Law 6.

## Integration Points

The runtime helpers (`jit_alloc`, element store) touch the heap; the
interpreter allocates through the same paths; safepoints poll a flag that
the current single-mutator design leaves cold (ticket CEP-ENGINE-011 covers
multi-mutator).

## Known Limitations

1. There is no old-generation collector yet; promotion is one-way and the
   old space grows until the budget refuses.
2. Interned strings are never collected (CEP-ENGINE-015).
3. The safepoint poll is an entry point for future multi-mutator support;
   today it is a no-op by design.

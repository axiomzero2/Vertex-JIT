# Runtime Helpers

## Abstract

`engine/src/hot/runtime.rs` implements the fixed ABI generated code uses to
reach the engine: the checked fmod helper, the division-by-zero and bounds
traps, allocation, element load and store, and the safepoint poll. The ABI
is System V AMD64; entries are `extern "C"` and never change silently.

## Formal Definitions

`with_trampoline(heap, code_base, entry)` registers a per-thread trampoline
state (jmp buffer, heap pointer, code base, frame pointer), calls the entry
and reports either `Returned(word)` or `Trapped(state)`. Trap entries
longjmp to the registered buffer; an unregistered trap aborts the process,
which is an engine bug made loud per Law 6.

## Algorithm

The fmod helper computes `fmod` and canonicalizes the result. The div0 trap
stores the trap kind and machine state into the thread-local, then longjmps.
Allocation returns zero on failure; the calling template traps on zero.
Element helpers bounds-check and return typed pairs; the bounds trap path
feeds the deopt machinery (doc/deopt.md).

## Invariants

1. The jmp buffer is sized above both glibc and musl x86-64 jmp_buf sizes;
   the constant is documented in the file header.
2. One trampoline per thread; nested guarded calls are not supported and
   the registration asserts it.
3. The frame-pointer capture assumes frame pointers are enabled; the crate
   never disables them.
4. Trap entries never return to generated code.

## Integration Points

Codegen embeds the helper addresses at lowering time (doc/codegen.md); the
deopt path validates the recorded code base against the engine's compiled
table; the safepoint poll is the future multi-mutator entry point
(doc/gc.md, ticket CEP-ENGINE-011).

## Known Limitations

1. The trampoline supports a single machine frame for deopt; cross-frame
   traps are refused (CEP-ENGINE-013).
2. The helpers trust the engine-validated arguments; they are not a
   security boundary against corrupted generated code.

# Code Generation: IR to x86-64

## Abstract

`engine/src/cold/codegen.rs` lowers the verified IR graph to x86-64 machine
code. Values live in rbp-relative slots allocated per node; control flow is
per-block emission with phi copies at block exits. The output is published
through the code cache with a deopt record for every uncommon-trap site.

## Formal Definitions

Slots: every node gets a slot; vector nodes take `VEC_SLOTS` consecutive
slots. The prologue reserves `next_slot * 8` bytes. Slots are
`[rbp - 8*(slot+1)]`.

Branch polarity: the builder records, per block, whether successor zero is
the condition-true side (`taken_sides`); lowering emits
`test cond; jz <false side>; jmp <true side>`.

Phi copies: at a block exit, for every successor with phis, the input
selected by this predecessor's index is copied into the phi's slot. Loop
headers select input 0 for the entry predecessor and input 1 for the latch;
ordinary merges select by predecessor index.

## Algorithm

Per block: schedule the block's nodes in dependency order (a node emits only
after its data inputs emitted; phis emit nothing), emit phi copies, then the
block terminator. The halt tail returns the bottom of the recorded value
stack with the site's type; the return tag is read from the deopt record.

Guards: DIV carries the zero guard and NaN canonicalization inline; array
accesses emit bounds guards with uncommon traps whose return addresses are
recorded for deopt reconstruction.

## Invariants

1. The verifier runs before lowering; lowering assumes dominating inputs.
2. Phi copy destinations are distinct slots, so sequential copies equal the
   parallel copy semantics.
3. Every trap site's return address is recorded before publication; deopt
   reconstructs from these records only.
4. Generated code calls the runtime only through the fixed helper addresses
   of `Helpers` (fmod, div0 trap, bounds trap, alloc, element load/store,
   safepoint poll).

## Integration Points

The engine lowers once per compiled function and publishes bytes plus the
deopt table. `doc/deopt.md` documents the reconstruction contract;
`doc/runtime.md` the helper ABI; `doc/value.md` the word/tag convention the
call sequence follows.

## Known Limitations

1. The vector path covers contiguous float64 loops with constant strides;
   other shapes take the scalar fallback.
2. Register allocation is slot-based memory traffic; a register allocator is
   future work and is not required by the current budgets.

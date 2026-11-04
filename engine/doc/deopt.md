# Deoptimization

## Abstract

When a guard fails inside compiled code, the uncommon trap transfers control
back to the engine trampoline through `setjmp`/`longjmp`, and
`engine/src/cold/deopt.rs` reconstructs interpreter frames at the trapping
bytecode pc. Deoptimization is the correctness backstop for every
speculative tier; a wrong reconstruction is a Severity 0 defect.

## Formal Definitions

A deopt site is `(node, DeoptSite { bc_pc, stack, locals })` recorded during
lifting and rewritten by passes. The stack list holds `(node, type)` pairs
bottom to top; the locals list holds `(slot, node, type)` per live variable.

A trap state is `(kind, rbp, rsp, ret, code_base)` captured by the runtime
trap entry. A trap is single-frame when the saved rbp chain terminates at the
trampoline's frame; multi-frame traps are refused because cross-frame
reconstruction is pending (ticket CEP-ENGINE-013).

## Algorithm

`reconstruct` walks the deopt record: every stack value is read from its
node's slot in machine memory and decoded with the recorded type; every
local is read the same way into the rebuilt frame. The new interpreter frame
resumes at `bc_pc` with the reconstructed operand stack, so the interpreter
continues the program exactly where the compiled frame was.

## Invariants

1. Every recorded operand dominates its site's block; the verifier enforces
   it after every pass. A body definition recorded at a loop header would
   read an unwritten slot on a trip-0 iteration; the header safepoint
   records phi nodes for exactly this reason.
2. Trap entries never return to generated code; they longjmp to the
   registered trampoline or abort loudly when unregistered.
3. The engine validates `code_base` against its own compiled table before
   using a trap state.
4. Deopted functions drop to the interpreter tier and re-enter the tiering
   ladder through the normal counters (spec 6.3).

## Integration Points

`hot::runtime` implements the trap entries and the trampoline
(`with_trampoline`); codegen records `DeoptRecord` tables at publication;
the engine performs reconstruction in `run_compiled` and swaps the tier.

## Known Limitations

1. Cross-frame deopt is pending (CEP-ENGINE-013); a trap under a machine
   call frame fails loudly instead of guessing.
2. Reconstruction materializes escaped objects on the slow path only;
   deopt-site array materialization interacts with PEA and is tracked
   (CEP-ENGINE-009).

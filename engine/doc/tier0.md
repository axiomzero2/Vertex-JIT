# Tier 0: Copy-and-Patch

## Abstract

Tier 0 translates bytecode to machine code by copying fixed x86-64 templates
and patching immediate holes. There is no IR and no analysis beyond the
legality gate. The target latency is under five microseconds per function
(spec 2.1), which the latency test asserts with slack for CI noise.

## Formal Definitions

A template is `(code, holes)` where `code` is fixed bytes and `holes` lists
`(offset, kind)` with kind `Imm64`, `Rel32` or `Disp8Slot`. The template
library (`TemplateLibrary`) holds one template per T0 opcode plus a depth
aware halt tail and a value-return tail.

The machine frame: the prologue pushes `rbp`, sets `rbp = rsp` and reserves
`locals * 8` bytes. Locals live at `[rbp - 8*(slot+1)]`. The value stack is
the machine stack; templates push and pop it.

## Algorithm

`compile` runs three passes. Pass one computes the byte offset of every pc
from the template sizes. Pass two copies templates into the buffer and
patches holes: immediates from the operand, rel32 from the target's offset
relative to the instruction end, disp8 from the slot. Pass three simulates
the value-stack depth and lattice types linearly and emits the halt tail
that returns the bottom of the stack with the static return tag.

Stack discipline: binary templates collapse their operand pair in place
(result into the left slot, `add rsp, 8`); comparisons do the same. A
template that pushed its result instead would leak sixteen bytes per
execution and overflow the machine stack inside loops; the golden test
`golden_compare_templates_collapse_operand_pair` pins the bytes and matrix A
in `tests/stack_discipline.rs` pins the behavior.

## Invariants

1. Template stack effects match the opcode semantics: push templates produce
   one value, binary templates consume two and produce one, branches consume
   the condition, `Drop(slot)` consumes nothing (it retires a binding).
2. The depth simulation treats an unconditional jump as killing the linear
   path up to its target; merge points are exact because `check_legality`
   enforces equal stack states at every merge.
3. Jump displacements are rel32 relative to the instruction end; both ends
   are computed in template coordinates, which the prefix cancels.
4. Publication is W^X: bytes are written into the RW window and sealed to RX
   (doc/code-cache.md).

## Integration Points

`check_legality` is the gate: it runs a dataflow over the bytecode and
rejects programs outside the subset (type-conflicting merges, shadowing
stores, unbound drops, stack deeper than the maximum). Refused programs stay
on the interpreter. The engine calls `tier0::compile` at tier selection and
publishes through the code cache.

## Known Limitations

1. The T0 subset rejects shadowing stores because the flat frame cannot
   restore an outer binding.
2. Function-like macro logic and self-modifying patterns are out of scope;
   the template set is closed.
3. Compile latency is asserted with generous slack; the budget lives in
   `tests/t0_exec.rs` and benches/EVIDENCE.md records measurements.

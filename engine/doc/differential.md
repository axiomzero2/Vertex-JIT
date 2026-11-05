# Differential Testing Contract

## Abstract

The interpreter is the reference implementation. Every tier must match it
bit-for-bit on the whole program corpus. `engine/tests/differential.rs`
defines the harness; `engine/tests/stack_discipline/` defines the
regression matrices required by standard 61.3 (fifty tests per confirmed
defect class).

## Formal Definitions

A differential case is a program assembled to legacy bytecode plus an
expectation that running it on `Tier::Interp`, `T0`, `T1`, `T2` and `T3`
yields the same exit value. Values compare with full `Value` equality:
word and tag both.

## Algorithm

For each case the harness builds one engine per tier with `JitBudgets::
default`, forces the tier, runs to the exit and compares. Tier refusals are
visible: the engine falls back by tiering down and the run still completes,
so a refusal that changes the result fails the case.

## Invariants

1. The corpus covers every arithmetic and comparison opcode, loops with
   every trip-count boundary class, nested loops, diamonds inside and
   outside loops, boolean values returned through loops, Drop shapes and
   branch-polarity variants.
2. Matrix A pins the comparison template stack collapse; matrix B pins the
   Drop depth simulation and the halt-tail lattice; matrix C pins the IR
   environment snapshots, loop-header phis, header safepoints and the
   unbalanced-branch refusal.
3. The matrices are exactly fifty cases each; the quota test enforces the
   count.
4. Golden byte tests pin the template encodings the matrices depend on.

## Integration Points

The engine's tier table is forced per run; no warm-up counters apply. The
case assembler mirrors the production decoder's conventions (instruction
index targets, first-use slots).

## Known Limitations

1. Programs with an empty value stack at Halt are outside the differential
   contract: the interpreter yields no value, the tiers yield zero
   (CEP-ENGINE-014).
2. The interpreter's single budget window bounds loop trip counts in the
   corpus; the deep-stress cases stay inside 20000 back edges, because a
   budget handoff is a tiering decision, not a value.

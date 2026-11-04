# Tiering Engine

## Abstract

`engine/src/cold/engine.rs` owns the tiered execution loop: it runs the
interpreter, counts invocations and back edges, selects tiers, compiles,
publishes, deoptimizes and re-selects. The tier thresholds are spec-mandated:
T0 below 10 calls, T1 at 10 to 100, T2 at 100 to 1000, T3 above 1000, OSR
at 20000 back edges.

## Formal Definitions

Tier selection is a function of the profile counters: per-function
invocations and per-pc back-edge counts. Budgets (`JitBudgets`) bound the
code cache, IR node counts, inlining depth and unroll factors; the engine
never exceeds a budget, it refuses and falls back.

## Algorithm

The run loop picks the top frame's function tier each iteration. The
interpreter path runs one budget window; on `BackEdgeBudget` the engine
counts the window, maybe compiles, and attempts OSR (doc/osr.md). Compiled
paths call through the trampoline; a returned word decodes with the static
tag; a trap reconstructs interpreter frames (doc/deopt.md) and drops the
function to the interpreter.

Compilation refusals (outside the T0 subset or the IR subset) leave the
function on the interpreter; the fallback is documented, not silent: the
tier table records it.

## Invariants

1. A function runs on exactly one tier per engine step; tier changes happen
   between steps, never concurrently.
2. Every compilation goes through the code cache with W^X publication.
3. Profile counters are monotonic within an engine run.
4. Deopt is the only path from compiled code back to the interpreter
   mid-function; everything else crosses at function boundaries.

## Integration Points

The differential suite (`tests/differential.rs`) drives this loop with
forced tiers and compares every tier against the interpreter bit-for-bit.
The regression matrices in `tests/stack_discipline.rs` pin the defect
classes found during development (comparison template stack collapse, Drop
depth modeling, IR environment snapshots, phi typing, dominance).

## Known Limitations

1. OSR transfers only at loop headers of functions the interpreter runs;
   compiled-code OSR is future work.
2. Inlining exists as a budget but the call graph is single-function per
   compilation; cross-function inlining is tracked with the T3 roadmap.

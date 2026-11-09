# Cross-Function Inlining (T3)

## Abstract

`engine/src/cold/inline.rs` implements the T3 inliner (spec 5.2.1): at a
T3 compilation, eligible call sites have the callee's body spliced into
the caller's bytecode before the lift. The merged function is plain
verified bytecode — it rides the normal lift/pass/codegen path, and it is
registered in the function table as the interpreter image a deopt resumes
in. Bytecode-level splicing keeps every tier above the transform honest:
there is no separate merged IR to verify, and the merged body's own
compilation degrades gracefully to the un-inlined body on any refusal.

## Formal Definitions

A call site is `Call` with operand `(argc << 32) | callee` (doc/bytecode.md).
A splice replaces the call with (a) `argc` `StoreVar` instructions binding
the popped arguments to the callee's remapped parameter slots and (b) the
callee's body with its trailing `Ret` removed — the returned value stays
on the stack as the call's result. The splice budget is the compilation's
`max_ir_nodes`; the depth budget and the callee node budget come from
`PassBudgets` (`max_inline_depth` = 4, `max_inline_nodes` = 500).

## Algorithm

`inline_calls` walks the caller once per depth level. At every call site
it applies the gates below; an accepted site splices the callee
(pre-inlined at depth − 1, so nested calls are resolved recursively) and
the walk continues with the abstract stack depth adjusted to the single
result. After the walk, jump targets are patched through the pc map.

The gates, in evaluation order:

1. **Stack depth**: the abstract stack below the arguments must be empty
   (`cur == argc`); spliced bodies reset to a zero baseline at block
   starts, so values under the arguments cannot be represented.
2. **Cycle**: a function never splices into its own call chain.
3. **Arity**: `argc` must fit the callee's locals (params bind
   `0..argc`).
4. **No Halt**: a Halt inside a spliced region would end the program from
   a call context.
5. **Liftable and in budget**: the callee must lift to the IR subset and
   hold `max_inline_nodes` (both the raw body and, after pre-inlining,
   the merged body).
6. **Single trailing Ret**: exactly one `Ret`, as the body's last
   instruction. Removing a mid-body Ret would glue the early-return path
   into the code that follows it; a branch edge into a Ret cannot lift
   (the edge would lack the return value), so every other shape refuses
   here and the call compiles un-inlined.
7. **Slot space**: merged locals stay below `MAX_LOCALS` (u16 operands).
8. **Splice budget**: total spliced instructions fit `max_ir_nodes`.

At the engine, a merge commits only when the merged body lifts and every
call left in it targets a compilable callee (tier above Interp —
`compile_callees_of` publishes exactly those before call addresses
resolve). A compilation whose final graph still calls an Interp-tier
callee refuses (unresolved address); the interpreter runs it exactly.

## Invariants

1. Every refused shape keeps interpreter parity: the un-inlined body (or
   the interpreter) computes the same result. Refusals change speed,
   never results.
2. The merged body passes `bytecode::verify` before registration and the
   IR verifier after the pipeline; a deopt resumes on bytecode whose pc
   and slot space match the compiled code (`CompiledFn::interp_fn`).
3. `stats.inlined_calls` counts every site in the merged body, at every
   depth, and only for compilations that actually published.
4. Cyclic call graphs never compile above the interpreter (no post order
   for call addresses); the static cycle check refuses before any work
   and the interpreter is exact.

## Integration Points

The engine drives the inliner only at T3 (`compile_optimized`); OSR
compilation lowers no call addresses and refuses call-bearing graphs
(interpretation continues). The lift builder emits `Param` nodes from the
engine-derived arity table (`JitEngine::arities`); codegen lowers `Param`
through the System V argument registers and `Call` through callee-first
published addresses. Deopt resumes in the registered merged image
(doc/deopt.md).

## Known Limitations

1. Early-return callees (mid-body `Ret`) never inline; they compile
   un-inlined when they lift and verify, and interpret otherwise.
2. A hot caller whose callee stayed at the interpreter tier compiles only
   after the callee's own promotion publishes an address; until then the
   caller interprets exactly. The tier ladder resolves this as counters
   grow.
3. Cyclic call graphs (self-recursion, mutual recursion) never compile at
   T1+; they interpret exactly.
4. OSR compilation does not inline and refuses call-bearing graphs.
5. `NewArray` allocation-failure traps carry no deopt record (the helper
   traps without a site), so an allocation deopt inside compiled code
   fails loud (DeoptFailed) rather than resuming; bounds and division
   guards deopt exactly. Tracked as follow-up work on the alloc path.

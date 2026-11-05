# Optimization Passes

## Abstract

`engine/src/cold/passes/` implements the versioned pass pipeline — one
pass per module (chapter 62.3.1), with `mod.rs` carrying the pipeline
constants and the name dispatcher. Tiers
select pipelines by name: T1 runs const-fold, local-vn, if-fold, phi-simp,
dce; T2 adds gvn, licm, pea; T3 runs the full pipeline including unroll and
vectorization gates. Every pass is a function over `(Graph, DeoptSites)`
with an explicit budget.

## Formal Definitions

A pass is legal when it preserves program semantics on the interpreter's
reference execution, preserves traps and deopt sites, and terminates within
its budget. Effectiveness is measured as node-count reduction for
simplification passes and as the documented speedup for transformation
passes (vectorization gates on a predicted speedup above 2x).

## Algorithm

- const-fold: folds pure nodes whose inputs are constants; DIV keeps the
  zero guard.
- local-vn: value numbering with a hash table. A replacement is applied
  only when the existing node dominates the use's block; cross-block equal
  values that do not dominate stay. The dominance gate is the fix for the
  trip-0 loop defect: a value defined in a body that does not execute has
  an unwritten slot at the use.
- if-fold: constant conditions remove the dead projection side.
- phi-simp: replaces phis whose inputs agree.
- gvn: global value numbering over the dominator tree with the same
  dominance requirement.
- licm: hoists loop-invariant pure nodes out of natural loops computed from
  back edges; Div and allocations are excluded (they trap or allocate).
- pea: scalar replacement and stack allocation for non-escaping arrays.
- dce: removes pure nodes unreachable from the terminators and deopt state.

## Invariants

1. The pipeline order is versioned and explicit; a tier's pipeline never
   changes silently.
2. Every pass re-runs `ir::verify` requirements: data inputs dominate uses,
   phis match predecessor counts, terminators stay unique.
3. Deopt sites are rewritten by the same rewrites as nodes; a site whose
   operands no longer dominate its block is a verifier error, not a
   fallback.

## Integration Points

The engine applies `passes_for_tier` inside `compile_optimized` and lowers
afterwards. `doc/ir.md` documents the graph invariants passes rely on;
`doc/value.md` the types the phi typing uses.

## Known Limitations

1. Unrolling and vectorization are gated but conservative; the scalar
   fallback is the default when the prediction is unproven.
2. The dominance computation is the iterative dataflow idom algorithm;
   unreachable blocks keep `usize::MAX` idoms and are ignored by passes.

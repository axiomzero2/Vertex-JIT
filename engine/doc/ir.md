# Sea of Nodes IR

## Abstract

`engine/src/cold/ir.rs` defines the Sea of Nodes graph the optimizer works
on: control nodes (Start, Loop, Region, If, projections, Halt, Return,
Safepoint, UncommonTrap) and data nodes (constants, arithmetic, comparisons,
Phi, Call, array nodes). The lifter (`lift`) translates verified bytecode
into a graph; the verifier (`verify`) gates every use of the graph.

## Formal Definitions

A node is `(kind, inputs, block, type)`. Node ids are creation order, which
is not execution order after pass rewrites; the codegen scheduler orders by
data dependencies per block. Phi nodes carry a slot index and one input per
predecessor of their block's head.

Blocks are maximal straight-line instruction ranges determined by leaders
(jump targets and branch fallthroughs). Loop headers are blocks targeted by
a back edge (target pc at or before the branch pc).

## Algorithm

The lifter scans blocks in index order and maintains per-block state: the
control chain, the operand stack and the variable environment
(definition node per slot). Edges record their environment at branch time;
the first edge's environment is restored when the block is scanned. Loop
headers allocate a Phi for every live slot with a placeholder second input;
`close_loop` patches the back-edge inputs and records the header safepoint
with the header-safe environment (phi nodes, not body definitions).

The operand stack must be empty at every branch; shapes carrying live values
across a branch are refused with `UnbalancedStack` and fall back to the
interpreter. This is a refusal, not a silent drop: the per-edge state has no
value stack, so continuing would discard operands silently.

## Invariants

1. After `lift`, every data input dominates its use. The verifier enforces
   this; the local-vn defect (a replacement that did not dominate its use,
   reading an unwritten slot in trip-0 loops) is the regression matrix C
   case.
2. A loop-carried variable's SSA value at the header is the header phi; the
   exit path and the header safepoint reference the phi, never a body
   definition.
3. Phis exist only where reaching definitions differ, except loop headers
   where every live slot gets one (the back-edge value arrives later).
4. Merge phis take their inputs' common type; a numeric default would
   mis-decode boolean joins at the halt tail.
5. The verifier rejects duplicate terminators and missing terminators.

## Integration Points

The engine lifts once per tier selection and runs the tier's pass pipeline
(doc/passes.md) before lowering (doc/codegen.md). Deopt sites are recorded
during lifting and rewritten by passes; `doc/deopt.md` documents their
contract.

## Known Limitations

1. The IR subset refuses I/O and legacy call opcodes; those stay on the
   interpreter (documented per the engine fallback).
2. Phi simplification does not yet peel trivial self-referencing loop phis;
   codegen skips self-copies.
3. Cross-block value numbering requires dominance; see doc/passes.md for
   the pass-level contracts.

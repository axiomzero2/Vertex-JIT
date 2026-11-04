# Vector Code Paths

## Abstract

The codegen vector path lowers contiguous float64 loops with constant
strides to AVX2 (and, where detected and profitable, AVX-512) code, with a
scalar fallback for every shape the gate refuses. The gate requires a
predicted speedup above 2x (spec 4.4.2) and refuses shapes whose
materialization would change deopt behavior.

## Formal Definitions

A vectorizable loop is a natural loop whose body computes a strided element
operation over an array reference with a constant stride and no aliasing
ambiguity the gate cannot discharge. The vector width is four float64 lanes
for AVX2.

## Algorithm

The gate inspects the loop body for the vector node forms (VecLoad,
VecAddConst, VecSubConst, VecMulConst) after the scalar passes ran. Accepted
loops lower to packed instructions with an epilogue for the remainder
lanes; refused loops keep the scalar lowering unchanged. Vector slots take
`VEC_SLOTS` consecutive slots in the frame layout.

## Invariants

1. The scalar fallback is always available; a refused vectorization never
   changes results, only speed.
2. Vector and scalar code agree bit-for-bit on the differential corpus; the
   tier23 tests cover the accepted shapes and the refusal gate.
3. Register pressure from vector code is bounded by the slot count the
   prologue reserves.

## Integration Points

The pass pipeline produces the vector node forms (doc/passes.md); codegen
emits them when `TargetConfig::avx2` is set (doc/target.md); the heap's
element helpers serve the guard paths (doc/gc.md).

## Known Limitations

1. The vector-loop expansion interacts with loop-phi arity in one shape and
   is gated off pending rework (CEP-ENGINE-012); the scalar fallback runs
   instead.
2. Only float64 arithmetic vectorizes; integer and string paths stay
   scalar.

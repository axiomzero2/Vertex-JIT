# Engine Documentation Index

This directory documents every component of the Vertex JIT engine. The
documentation follows one structure: title, abstract, formal definitions,
algorithm, invariants, integration points, known limitations. A document is
committed in the same change as the code it describes.

Documents:

| Document | Component |
|---|---|
| [bytecode.md](bytecode.md) | Bytecode decoder, verifier and function model |
| [value.md](value.md) | Word representation, type tags and canonicalization |
| [tier0.md](tier0.md) | Tier 0 copy-and-patch templates and the legality gate |
| [code-cache.md](code-cache.md) | W^X executable code cache |
| [ir.md](ir.md) | Sea of Nodes IR: graph, builder, verifier |
| [passes.md](passes.md) | Optimization pass pipeline per tier |
| [inline.md](inline.md) | Cross-function inlining at T3 |
| [codegen.md](codegen.md) | IR to x86-64 lowering and runtime ABI |
| [deopt.md](deopt.md) | Deoptimization and frame reconstruction |
| [gc.md](gc.md) | GC heap, barriers and safepoints |
| [engine.md](engine.md) | Tiering engine, budgets and the run loop |
| [osr.md](osr.md) | On-stack replacement |
| [differential.md](differential.md) | Differential testing contract |
| [runtime.md](runtime.md) | Runtime helpers called from generated code |
| [target.md](target.md) | Target detection and feature gating |
| [vectorize.md](vectorize.md) | Vector code paths |

Rules for this directory:

1. Every claim about behavior is testable and is tested; the document names
   the test or the benchmark that pins it.
2. No marketing language. No exclamation marks. No emoji.
3. Known limitations are listed, not hidden; each carries a ticket of the
   form CEP-ENGINE-<n>.
4. A change to a contract updates the document and the tests in the same
   commit.

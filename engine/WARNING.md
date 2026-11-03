# Engine Notice

consult axiomzero2 before touching, this is sensitive, and will break easily. discord is axiomzero0 on the vertex server

## Scope of this directory

The `engine/` directory contains the Vertex JIT compiler: the tiered
execution engine (Tier 0 through Tier 3), the Sea of Nodes IR, the
deoptimization and on-stack replacement machinery and the garbage collector.

Changes here can invalidate the machine-code templates, the deoptimization
frame contract, the GC barrier and safepoint contracts and the value
representation shared with the interpreter. Read `doc/` before modifying
anything; the contracts are enforced by tests that fail loudly.

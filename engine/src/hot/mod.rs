// CEP:FILE: engine/src/hot/mod.rs
// CEP:WHAT: Hot module group: CEP-0 code executed per program operation.
// CEP:WHY: Standard sections 7.1 and 32.2: hot and cold code are separated
//          at file granularity so hot code stays auditable.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: compile-time only.
// CEP:EVIDENCE: tools/cep_lint.py enforces the CEP-0 token ban here.
// CEP:SECURITY: every module is reachable from generated machine code or
//               the interpreter dispatch loop; allocation and panics are
//               banned and linted.

/// Machine value representation and bounded stacks.
pub mod value;
/// Fixed-capacity LIFO value stack.
pub mod stack;
/// Baseline interpreter: construction surface, profiler, budgets.
pub mod interp;
/// The interpreter dispatch loop.
pub mod interp_run;
/// Interpreter frame machinery and stack primitives.
pub mod frame;
/// Host I/O hooks injected by the embedder.
pub mod host;
/// Generational GC heap fast paths: allocation, bounds-checked access,
/// write barriers.
pub mod heap;
/// Heap-side collection primitives: marks, sweeps, walks, root filtering.
pub mod heap_collect;
/// One mmap'd generation arena: bump/free-list allocation, start bitmap.
pub mod arena;
/// The uncommon-trap machinery and trampoline.
pub mod trap;
/// Runtime helpers called from generated machine code.
pub mod runtime;

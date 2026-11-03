// CEP:FILE: engine/src/cold/mod.rs
// CEP:WHAT: Cold module group: CEP-1/HPC compilation and orchestration.
// CEP:WHY: Standard sections 7.1 and 32.2: cold code (decoders, IR, passes,
//          codegen, engine driver) is grouped here and may allocate within
//          budgets.
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: compile-time only.
// CEP:EVIDENCE: cargo test.
// CEP:SECURITY: this group owns all untrusted-input handling; hot code
//               never sees unvalidated operands.

/// Normalized bytecode, the Vertex binary decoder, verifier and pool.
pub mod bytecode;
/// Engine error taxonomy (Law 6).
pub mod errors;
/// Tier definitions, thresholds, budgets (spec 6.3, standard 38.39).
pub mod tier;
/// Bounded W^X code cache with size-class pooling.
pub mod code_cache;
/// Tier 0 copy-and-patch compiler and the x86-64 emitter (spec 2).
pub mod tier0;
/// Sea of Nodes IR: graph, builder, verifier (spec 3.1, standard 38.17).
pub mod ir;
/// Optimization passes (spec 3.2, 4, 5; standard 38.20).
pub mod passes;
/// Machine-code lowering from the IR.
pub mod codegen;
/// Deoptimization metadata and frame reconstruction (spec 5.7).
pub mod deopt;
/// GC driver: roots, young collections, concurrent old marking.
pub mod gc;
/// The tiering engine driver (spec 6.3).
pub mod engine;
/// The versioned pass pipeline descriptor (38.20).
pub mod pipeline;

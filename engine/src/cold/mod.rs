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
// pub mod ir;  // added when implemented
// pub mod passes;  // added when implemented
// pub mod codegen;  // added when implemented
// pub mod deopt;  // added when implemented
// pub mod gc;  // added when implemented
// pub mod engine;  // added when implemented
// pub mod pipeline;  // added when implemented

// CEP:FILE: engine/src/lib.rs
// CEP:WHAT: Crate root of the Vertex tiered JIT engine.
// CEP:WHY: Single entry point wiring bytecode ingestion, tiered compilation
//          (T0-T3), deoptimization, on-stack replacement and the garbage
//          collector into one audited facade; the module tree makes the
//          hot/cold/target separation visible (standard 7.1, 32.2).
// CEP:CLASS: CC
// CEP:STATUS: complete
// CEP:FAILURE: none; module definitions cannot fail.
// CEP:ASSUMES: x86-64 Linux for code generation and execution; enforced by
//              platform::TargetConfig::detect at engine construction.
// CEP:COST: compile-time only; no runtime instructions.
// CEP:EVIDENCE: cargo test (unit + differential suites).
// CEP:SECURITY: Bytecode inputs are untrusted; decoders validate before
//               use. Generated code is engine-authored only; the code
//               cache enforces W^X (cold::code_cache).
//! # vertex-jit
//!
//! Tiered JIT execution engine for Vertex bytecode: T0 copy-and-patch
//! (spec 2), T1 Sea of Nodes with basic optimizations (spec 3), T2
//! GVN/PEA/LICM (spec 4), T3 PGO/inlining/unrolling/vectorization (spec 5),
//! OSR (spec 6) and a generational concurrent GC (spec 7).
//!
//! Layout follows CEP&CC section 32: `hot/` is CEP-0 code executed per
//! program operation; `cold/` is CEP-1/HPC compilation and orchestration;
//! `platform.rs` holds the named target configuration (Law 7).
#![forbid(unsafe_op_in_unsafe_fn)]

/// CEP-0 hot code: executed per program operation.
pub mod hot;
/// CEP-1/HPC compilation and orchestration code.
pub mod cold;
/// Named target configuration and runtime feature detection (Law 7).
pub mod platform;

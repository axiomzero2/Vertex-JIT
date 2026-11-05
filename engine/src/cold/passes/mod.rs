// CEP:FILE: engine/src/cold/passes/mod.rs
// CEP:WHAT: The pass-module umbrella: the versioned pipeline constants,
//           per-tier pass lists, the budget knobs and the name
//           dispatcher; one pass per submodule (chapter 62.3.1).
// CEP:WHY: Spec sections 3.2, 4.1-4.4 and 5.1-5.4; standard 38.20 requires
//          an explicit versioned pipeline where every pass declares
//          legality, analyses, invalidations and cost.
// CEP:CLASS: HPC-1
// CEP:STATUS: complete
// CEP:FAILURE: run_pass fails closed on unknown pass names; every pass
//              returns Result and the engine falls back to a lower tier
//              on failure. Passes re-verify through ir::verify at
//              pipeline checkpoints (38.18).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic; ordered arenas and BTreeMap keys.
// CEP:HPC-COMPILE-COST: dispatch plus the pass; per-pass costs are in the
//             submodule headers and benches/EVIDENCE.md.
// CEP:ASSUMES: graphs passed ir::verify; profile data from the interpreter.
// CEP:COST: see CEP:HPC-COMPILE-COST.
// CEP:EVIDENCE: tests/ir_passes.rs (per-pass legality, positive and
//               negative), tests/differential.rs (all tiers bit-identical).
// CEP:SECURITY: passes never remove bounds guards unless the guard cannot
//               fire (proved constants); see the pea and vectorize
//               contracts.


use crate::cold::ir::{self, DeoptSite, Graph, NodeId};

/// Constant folding over proved constants.
pub mod const_fold;
/// Dead code elimination.
pub mod dead_code_elimination;
/// Global value numbering.
pub mod global_value_numbering;
/// If folding on proved-constant conditions.
pub mod if_fold;
/// Loop invariant code motion.
pub mod licm;
/// Dominance-gated local value numbering.
pub mod local_value_numbering;
/// Partial escape analysis.
pub mod pea;
/// Trivial-phi elimination.
pub mod phi_simplify;
/// Cross-pass graph rewrites (node and deopt-site replacement).
pub mod rewrite;
/// Counted-loop unrolling under the pass budget.
pub mod unroll;
/// Static-trip vectorization of map loops.
pub mod vectorize;

/// The canonical versioned pipeline order (standard 38.20).
pub const PIPELINE_V1: &[&str] = &[
    "const-fold",
    "local-vn",
    "if-fold",
    "phi-simp",
    "dce",
    "gvn",
    "licm",
    "pea",
    "unroll",
    "vectorize",
    "inline",
    "const-fold",
    "dce",
];

/// Tier-to-pass-list mapping (spec 3.2, 4, 5; standard 15.2).
///
/// CEP:WHAT: Tier-to-pass mapping.
/// CEP:WHY: Explicit and versioned pipeline per tier.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: tests/ir_passes.rs pipeline tests.
/// CEP:SECURITY: none
#[must_use]
pub fn passes_for_tier(tier: u8) -> &'static [&'static str] {
    match tier {
        1 => &["const-fold", "local-vn", "if-fold", "phi-simp", "dce"],
        2 => &[
            "const-fold",
            "local-vn",
            "if-fold",
            "phi-simp",
            "dce",
            "gvn",
            "licm",
            "pea",
        ],
        _ => PIPELINE_V1,
    }
}

/// Pass budgets (spec 5.2.1, 5.3.1, 4.4.2).
#[derive(Debug, Clone, Copy)]
pub struct PassBudgets {
    /// Maximum unroll factor (spec 5.3.1).
    pub max_unroll_factor: u32,
    /// Minimum predicted speedup for vectorization (spec 4.4.2: >2x).
    pub vector_min_speedup: f64,
    /// Vector width in f64 lanes (4 for AVX2).
    pub vector_width: u32,
    /// Maximum inlining depth (spec 5.2.1: 3-5).
    pub max_inline_depth: u32,
    /// Maximum nodes of an inlinable callee body (spec 5.2.1: 500).
    pub max_inline_nodes: u32,
    /// Maximum static trip count the vectorizer/unroller will expand.
    pub max_static_trip: u32,
}

impl Default for PassBudgets {
    fn default() -> Self {
        PassBudgets {
            max_unroll_factor: 4,
            vector_min_speedup: 2.0,
            vector_width: 4,
            max_inline_depth: 4,
            max_inline_nodes: 500,
            max_static_trip: 4096,
        }
    }
}

/// Run one named pass.
///
/// CEP:WHAT: Pass dispatcher.
/// CEP:WHY: The pipeline references passes by name (38.20).
/// CEP:STATUS: complete
/// CEP:FAILURE: Unknown names fail closed.
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:ASSUMES: graph verified before the call.
/// CEP:COST: dispatch plus the pass.
/// CEP:EVIDENCE: tests/ir_passes.rs.
/// CEP:SECURITY: unknown pass names never run.
///
/// # Errors
/// Error string on pass failure or unknown name.
pub fn run_pass(
    name: &str,
    g: &mut Graph,
    sites: &mut Vec<(NodeId, DeoptSite)>,
    budgets: &PassBudgets,
) -> Result<(), String> {
    match name {
        "const-fold" => const_fold::const_fold(g, sites),
        "local-vn" => local_value_numbering::local_value_numbering(g, sites),
        "if-fold" => if_fold::if_fold(g, sites),
        "phi-simp" => phi_simplify::phi_simplify(g, sites),
        "dce" => dead_code_elimination::dead_code_elimination(g, sites),
        "gvn" => global_value_numbering::global_value_numbering(g, sites),
        "licm" => licm::licm(g),
        "pea" => pea::pea(g, sites),
        "unroll" => unroll::unroll(g, sites, budgets),
        "vectorize" => vectorize::vectorize(g, sites, budgets),
        "inline" => Ok(()), // driven by the engine at lift time; see engine.rs
        "verify" => ir::verify(g),
        _ => Err(format!("unknown pass: {name}")),
    }
}

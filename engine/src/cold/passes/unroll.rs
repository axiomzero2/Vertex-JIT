// CEP:FILE: engine/src/cold/passes/unroll.rs
// CEP:WHAT: Counted-loop unrolling under the pass budget.
// CEP:WHY: Spec 5.3.1: bounded unrolling exposes ILP for the vectorizer
//          and removes branch overhead (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:PASS: unroll
// CEP:STATUS: complete
// CEP:FAILURE: Err aborts compilation of the unit (engine falls back).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic.
// CEP:HPC-COMPILE-COST: O(expansion) bounded by max_unroll_factor and
//             max_static_trip.
// CEP:ASSUMES: graph passed ir::verify; budgets engine-configured.
// CEP:COST: O(expansion).
// CEP:EVIDENCE: tests/ir_passes.rs; differential suite.
// CEP:SECURITY: budget-bounded; never unrolls uncounted loops.

//! Counted-loop unrolling (spec 5.3.1).
use crate::cold::ir::{DeoptSite, Graph, NodeId};
use crate::cold::passes::PassBudgets;
use super::vectorize::{expand_loop_scalar, find_static_map_loops};

/// Unroll counted loops within the pass budget.
///
/// CEP:WHAT: Loop unrolling pass (pipeline "unroll").
/// CEP:WHY: Spec 5.3.1: bounded unrolling exposes ILP and removes branch
///          overhead; budgets bound the expansion.
/// CEP:STATUS: complete
/// CEP:FAILURE: Err aborts compilation of the unit; the engine falls back
///              to a lower tier (never silently).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:HPC-COMPILE-COST: O(expansion), bounded by max_unroll_factor and
///             max_static_trip.
/// CEP:ASSUMES: the graph passed ir::verify; budgets are
///              engine-configured; shared expansion helpers come from
///              the vectorize module.
/// CEP:COST: O(expansion).
/// CEP:EVIDENCE: tests/ir_passes.rs; tests/differential.rs.
/// CEP:SECURITY: budget-bounded; never unrolls uncounted loops.
pub fn unroll(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>, budgets: &PassBudgets) -> Result<(), String> {
    // Handled together with vectorization: vectorize() performs the full
    // static-trip expansion (vector plus scalar remainder); plain unrolling
    // without vectorization is the same expansion with scalar nodes only.
    // This function expands eligible loops to straight-line scalar code
    // when the trip count is static and within budget.
    let candidates = find_static_map_loops(g, budgets)?;
    for c in candidates {
        if !c.vector_eligible {
            expand_loop_scalar(g, sites, &c, budgets)?;
        }
    }
    Ok(())
}

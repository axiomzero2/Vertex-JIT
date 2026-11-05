// CEP:FILE: engine/src/cold/passes/phi_simplify.rs
// CEP:WHAT: Trivial-phi elimination (single distinct input or all inputs
//           equal).
// CEP:WHY: Spec 4.4: trivial phis block value numbering and LICM;
//          collapsing them early shrinks every later pass (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:PASS: phi-simp
// CEP:STATUS: complete
// CEP:FAILURE: Err aborts compilation of the unit (engine falls back).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic.
// CEP:HPC-COMPILE-COST: O(n).
// CEP:ASSUMES: graph passed ir::verify.
// CEP:COST: O(n).
// CEP:EVIDENCE: tests/ir_passes.rs; differential suite.
// CEP:SECURITY: none

//! Trivial-phi elimination (spec 4.4).
use super::rewrite::replace_all;
use crate::cold::ir::{DeoptSite, Graph, NodeId, NodeKind};
use super::dead_code_elimination::dead_code_elimination;

/// Collapse trivial phis (one distinct input).
///
/// CEP:WHAT: Phi simplification pass (pipeline "phi-simp").
/// CEP:WHY: Spec 4.4: trivial phis block value numbering and LICM;
///          collapsing them early shrinks every later pass.
/// CEP:STATUS: complete
/// CEP:FAILURE: Err aborts compilation of the unit; the engine falls back
///              to a lower tier (never silently).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:HPC-COMPILE-COST: O(n).
/// CEP:ASSUMES: the graph passed ir::verify.
/// CEP:COST: O(n).
/// CEP:EVIDENCE: tests/ir_passes.rs; tests/differential.rs.
/// CEP:SECURITY: none
pub fn phi_simplify(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>) -> Result<(), String> {
    for i in 0..g.nodes.len() {
        let NodeKind::Phi { .. } = g.nodes[i].kind else { continue };
        let inputs = g.nodes[i].inputs.clone();
        if !inputs.is_empty() && inputs.iter().all(|v| *v == inputs[0]) {
            let repl = inputs[0];
            replace_all(g, sites, NodeId(i as u32), repl);
        }
    }
    dead_code_elimination(g, sites)
}

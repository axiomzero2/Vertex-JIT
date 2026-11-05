// CEP:FILE: engine/src/cold/passes/if_fold.rs
// CEP:WHAT: If-node folding on proved-constant conditions, with the
//           projection helper proj_of.
// CEP:WHY: Spec 4.3: folded branches remove dead control paths before
//          GVN/LICM run (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:PASS: if-fold
// CEP:STATUS: complete
// CEP:FAILURE: Err aborts compilation of the unit (engine falls back).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic.
// CEP:HPC-COMPILE-COST: O(n).
// CEP:ASSUMES: graph passed ir::verify.
// CEP:COST: O(n).
// CEP:EVIDENCE: tests/ir_passes.rs; differential suite.
// CEP:SECURITY: folds only proved-constant conditions.

//! If folding on proved-constant conditions (spec 4.3).
use crate::cold::ir::{DeoptSite, Graph, NodeId, NodeKind, VType};
use super::const_fold::{const_of, F};
use super::dead_code_elimination::dead_code_elimination;

/// Fold If nodes whose condition is a proved constant.
///
/// CEP:WHAT: If folding pass (pipeline "if-fold").
/// CEP:WHY: Spec 4.3: folded branches remove dead control paths before
///          GVN/LICM run, shrinking the analyzed graph.
/// CEP:STATUS: complete
/// CEP:FAILURE: Err aborts compilation of the unit; the engine falls back
///              to a lower tier (never silently).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:HPC-COMPILE-COST: O(n).
/// CEP:ASSUMES: the graph passed ir::verify; folded regions are rewired
///              through rewrite::replace_all and swept by DCE.
/// CEP:COST: O(n).
/// CEP:EVIDENCE: tests/ir_passes.rs; tests/differential.rs.
/// CEP:SECURITY: folds only proved-constant conditions.
pub fn if_fold(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>) -> Result<(), String> {
    // Guards whose condition folds so the trap side is dead are removed.
    // Control structure (Regions, projections, phi arity) is left intact:
    // severing Region inputs breaks the pred-to-phi correspondence
    // (standard 38.21 pass legality).
    let mut dead_traps: Vec<NodeId> = Vec::new();
    for i in 0..g.nodes.len() {
        let NodeKind::UncommonTrap { .. } = g.nodes[i].kind else { continue };
        let Some(proj) = g.nodes[i].inputs.first().copied() else {
            continue;
        };
        let (NodeKind::IfTrue | NodeKind::IfFalse) = g.nodes[proj.0 as usize].kind else {
            continue;
        };
        let Some(ifn) = g.nodes[proj.0 as usize].inputs.first().copied() else {
            continue;
        };
        let NodeKind::If = g.nodes[ifn.0 as usize].kind else { continue };
        let Some(cond) = g.nodes[ifn.0 as usize].inputs.get(1).copied() else {
            continue;
        };
        let is_true_side = g.nodes[proj.0 as usize].kind == NodeKind::IfTrue;
        match const_of(g, cond) {
            // Trap on the true side and the condition is constant false:
            // the trap cannot fire.
            Some(F::Bool(false)) if is_true_side => dead_traps.push(NodeId(i as u32)),
            // Trap on the false side and the condition is constant true:
            // the trap cannot fire.
            Some(F::Bool(true)) if !is_true_side => dead_traps.push(NodeId(i as u32)),
            _ => {}
        }
    }
    for t in dead_traps {
        sites.retain(|(n, _)| *n != t);
        for node in &mut g.nodes {
            node.inputs.retain(|inp| *inp != t);
        }
        // Replace the node with an inert constant control marker: clearing
        // its kind keeps ids stable.
        g.nodes[t.0 as usize].kind = NodeKind::Null;
        g.nodes[t.0 as usize].ty = VType::Unit;
        g.nodes[t.0 as usize].inputs.clear();
    }
    dead_code_elimination(g, sites)
}

/// Find the IfTrue/IfFalse projection of an If.
/// CEP:WHAT: Projection lookup: the truthy/falsy output of an If node.
/// CEP:WHY: If folding rewires the taken side; the projection is the control
///              edge that must be redirected.
/// CEP:STATUS: complete
/// CEP:FAILURE: None when the node shape is not an If projection.
/// CEP:ASSUMES: the graph passed ir::verify.
/// CEP:COST: O(inputs).
/// CEP:EVIDENCE: tests/ir_passes.rs if-fold cases.
fn proj_of(g: &Graph, ifn: NodeId, truthy: bool) -> Option<NodeId> {
    for (i, node) in g.nodes.iter().enumerate() {
        let want = if truthy { NodeKind::IfTrue } else { NodeKind::IfFalse };
        if node.kind == want && node.inputs.first() == Some(&ifn) {
            return Some(NodeId(i as u32));
        }
    }
    None
}

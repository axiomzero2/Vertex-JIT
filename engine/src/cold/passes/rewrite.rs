// CEP:FILE: engine/src/cold/passes/rewrite.rs
// CEP:WHAT: Cross-pass graph rewrites: node-use replacement and deopt-site
//           replacement after nodes merge or die.
// CEP:WHY: Every simplification pass funnels replacements through these
//          two primitives; keeping them apart from the pass logic keeps
//          the rewrite invariants auditable (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:STATUS: complete
// CEP:FAILURE: none (pure rewrites over verified graphs).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic.
// CEP:ASSUMES: `from` is dead or about to die; sites lists are engine-owned.
// CEP:COST: O(uses) and O(sites) per rewrite.
// CEP:EVIDENCE: differential tests (deopt after optimization).
// CEP:SECURITY: none

//! Cross-pass graph rewrites.
use crate::cold::ir::{DeoptSite, Graph, NodeId, VType};

/// CEP:ASSUMES: none
/// CEP:COST: O(sites * stack).
/// CEP:EVIDENCE: differential tests (deopt after optimization).
/// CEP:SECURITY: none
/// CEP:WHAT: Rewrite deopt-site references after a node merge.
/// CEP:WHY: Sites must reference live nodes after every pass or deopt
///              materialization reads freed nodes.
/// CEP:STATUS: complete
/// CEP:FAILURE: none (pure rewrite).
/// CEP:ASSUMES: `to` is the surviving representative of `from`.
/// CEP:COST: O(sites).
/// CEP:EVIDENCE: differential tests (deopt after optimization).
pub(crate) fn sites_replace(sites: &mut [(NodeId, DeoptSite)], from: NodeId, to: NodeId) {
    for (n, s) in sites.iter_mut() {
        if *n == from {
            *n = to;
        }
        for e in s.stack.iter_mut() {
            if e.0 == from {
                e.0 = to;
            }
        }
        for l in s.locals.iter_mut() {
            if l.1 == from {
                l.1 = to;
            }
        }
    }
}

// ---------------------------------------------------------------- T1 passes

/// Constant folding over pure binary nodes.
///
/// CEP:HPC-PASS-KIND: local rewrite.
/// CEP:HPC-PASS-INPUT: verified graph.
/// CEP:HPC-PASS-OUTPUT: folded constants; identical semantics.
/// CEP:HPC-PASS-LEGALITY: pure nodes only; Div folds only for nonzero
///             constant divisors (the trap must stay reachable otherwise);
///             NaN results canonicalized (doc/value.md).
/// CEP:HPC-PASS-PRESERVES: value semantics, traps, deopt sites.
/// CEP:HPC-PASS-COST: O(n).
/// CEP:HPC-PASS-EVIDENCE: tests/ir_passes.rs const_fold cases.
///
/// # Errors
/// Internal inconsistency.
/// CEP:WHAT: Replace every use of `from` with `to` and follow replacements in
///              deopt sites.
/// CEP:WHY: Node merges (GVN, folding) must retarget all uses in one
///              primitive so no pass forgets a site.
/// CEP:STATUS: complete
/// CEP:FAILURE: none (pure rewrite).
/// CEP:ASSUMES: `from` becomes dead after the rewrite (DCE confirms).
/// CEP:COST: O(uses + sites).
/// CEP:EVIDENCE: tests/ir_passes.rs; differential suite.
pub(crate) fn replace_all(g: &mut Graph, sites: &mut [(NodeId, DeoptSite)], from: NodeId, to: NodeId) {
    // Control inputs (Region/If/Loop operands) are topology: replacing
    // them with a data value breaks the pred-to-phi correspondence.
    // Passes that rewrite control must do so with explicit block surgery.
    let control_inputs: Vec<usize> = g
        .nodes
        .iter()
        .enumerate()
        .flat_map(|(i, n)| {
            n.inputs
                .iter()
                .filter(|inp| {
                    **inp == from && g.nodes[inp.0 as usize].ty == VType::Control
                })
                .map(move |_| i)
        })
        .collect();
    for node in &mut g.nodes {
        for inp in &mut node.inputs {
            if *inp == from {
                *inp = to;
            }
        }
    }
    for i in control_inputs {
        let idx = g.nodes[i]
            .inputs
            .iter()
            .position(|inp| *inp == to)
            .expect("control input just rewritten");
        g.nodes[i].inputs[idx] = from;
    }
    sites_replace(sites, from, to);
}

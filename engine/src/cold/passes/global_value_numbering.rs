// CEP:FILE: engine/src/cold/passes/global_value_numbering.rs
// CEP:WHAT: Global value numbering over dominator-tree-scoped hash keys.
// CEP:WHY: Spec 5.1: GVN removes cross-block redundancies after the T1
//          cleanup passes (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:PASS: gvn
// CEP:STATUS: complete
// CEP:FAILURE: Err aborts compilation of the unit (engine falls back).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic (ordered keys).
// CEP:HPC-COMPILE-COST: O(n log n) (GVN string keys).
// CEP:ASSUMES: graph passed ir::verify.
// CEP:COST: O(n log n).
// CEP:EVIDENCE: tests/ir_passes.rs; tests/tier23_tests.rs; differential
//               suite.
// CEP:SECURITY: none

//! Global value numbering (spec 5.1).
use super::rewrite::replace_all;
use crate::cold::ir::{self, DeoptSite, Graph, NodeId, NodeKind};
use super::dead_code_elimination::dead_code_elimination;
use super::local_value_numbering::node_key;
use std::collections::BTreeMap;

/// Number values across blocks under dominator scoping.
///
/// CEP:WHAT: Global value numbering pass (pipeline "gvn").
/// CEP:WHY: Spec 5.1: GVN removes cross-block redundancies after the T1
///          cleanup passes; keys are ordered for determinism.
/// CEP:STATUS: complete
/// CEP:FAILURE: Err aborts compilation of the unit; the engine falls back
///              to a lower tier (never silently).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic (ordered keys).
/// CEP:HPC-COMPILE-COST: O(n log n) (GVN string keys).
/// CEP:ASSUMES: the graph passed ir::verify.
/// CEP:COST: O(n log n).
/// CEP:EVIDENCE: tests/ir_passes.rs; tests/tier23_tests.rs; differential
///               suite.
/// CEP:SECURITY: none
pub fn global_value_numbering(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>) -> Result<(), String> {
    let doms = ir::idoms(g);
    let mut table: BTreeMap<String, NodeId> = BTreeMap::new();
    for i in 0..g.nodes.len() {
        let node = &g.nodes[i];
        if !Graph::is_pure(&node.kind) || matches!(node.kind, NodeKind::Phi { .. }) {
            continue;
        }
        let key = node_key(g, NodeId(i as u32));
        if let Some(existing) = table.get(&key) {
            let b = node.block.0 as usize;
            let eb = g.nodes[existing.0 as usize].block.0 as usize;
            if ir::dominates(&doms, eb, b) {
                let repl = *existing;
                replace_all(g, sites, NodeId(i as u32), repl);
                continue;
            }
        }
        table.insert(key, NodeId(i as u32));
    }
    dead_code_elimination(g, sites)
}

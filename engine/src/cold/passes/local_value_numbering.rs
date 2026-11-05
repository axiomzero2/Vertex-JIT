// CEP:FILE: engine/src/cold/passes/local_value_numbering.rs
// CEP:WHAT: Local (single-block, dominance-gated) value numbering with the
//           node_key normalizer.
// CEP:WHY: Spec 4.2: dominance-gated local VN removes redundant
//          computations inside blocks without cross-block hazards
//          (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:PASS: local-vn
// CEP:STATUS: complete
// CEP:FAILURE: Err aborts compilation of the unit (engine falls back).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic (BTreeMap keys).
// CEP:HPC-COMPILE-COST: O(n log n) over block-local nodes.
// CEP:ASSUMES: graph passed ir::verify; the dominance gate runs before
//              any replacement.
// CEP:COST: O(n log n).
// CEP:EVIDENCE: tests/ir_passes.rs (incl. trip-0 poisoning negatives);
//               differential suite.
// CEP:SECURITY: replacements respect the dominance gate.

//! Dominance-gated local value numbering (spec 4.2).
use super::rewrite::replace_all;
use crate::cold::ir::{self, DeoptSite, Graph, NodeId, NodeKind};
use super::const_fold::kind_name;
use super::dead_code_elimination::dead_code_elimination;
use std::collections::BTreeMap;

/// Number values block-locally under the dominance gate.
///
/// CEP:WHAT: Local value numbering pass (pipeline "local-vn").
/// CEP:WHY: Spec 4.2: dominance-gated local VN removes redundant
///          computations inside blocks; the gate keeps cross-block uses
///          sound (trip-0 regression, matrix C).
/// CEP:STATUS: complete
/// CEP:FAILURE: Err aborts compilation of the unit; the engine falls back
///              to a lower tier (never silently).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic (BTreeMap keys).
/// CEP:HPC-COMPILE-COST: O(n log n) over block-local nodes.
/// CEP:ASSUMES: the graph passed ir::verify; the dominance gate runs
///              before any replacement.
/// CEP:COST: O(n log n).
/// CEP:EVIDENCE: tests/ir_passes.rs (incl. trip-0 poisoning negatives);
///               tests/stack_discipline/matrix_c.rs; differential suite.
/// CEP:SECURITY: replacements respect the dominance gate.
pub fn local_value_numbering(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>) -> Result<(), String> {
    let doms = ir::idoms(g);
    let mut table: BTreeMap<String, NodeId> = BTreeMap::new();
    for i in 0..g.nodes.len() {
        let node = &g.nodes[i];
        if !Graph::is_pure(&node.kind) || matches!(node.kind, NodeKind::Phi { .. }) {
            continue;
        }
        let key = node_key(g, NodeId(i as u32));
        if let Some(existing) = table.get(&key) {
            // The replacement must dominate the use's block. A value only
            // defines the same computation on paths where it executes; a
            // block skipped at runtime (a trip-0 loop body) leaves the
            // slot unwritten and the use reads stale memory. Cross-block
            // equal values that do not dominate stay in the table.
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

/// Canonical key for a pure node: kind plus input ids (stable within a
/// compilation unit, standard 38.17).
/// CEP:WHAT: Canonical value key for one node (opcode + normalized inputs).
/// CEP:WHY: Value numbering compares keys, not nodes; ordered keys keep the
///              pass deterministic (6.5).
/// CEP:STATUS: complete
/// CEP:FAILURE: none (pure function of the node).
/// CEP:ASSUMES: input ids are already replaced to their values.
/// CEP:COST: O(inputs).
/// CEP:EVIDENCE: tests/ir_passes.rs lvn cases.
pub(crate) fn node_key(g: &Graph, n: NodeId) -> String {
    let node = &g.nodes[n.0 as usize];
    let mut s = String::new();
    s.push_str(kind_name(&node.kind));
    s.push('(');
    // Constants are distinguished by their payload, not just their kind:
    // two ConstNum nodes are equal only when their values match.
    match &node.kind {
        NodeKind::ConstNum(f) => s.push_str(&format!("{}:{f:?}", "")),
        NodeKind::ConstBool(b) => s.push_str(&format!("{b}")),
        NodeKind::ConstStr(c) => s.push_str(&format!("{c}")),
        _ => {
            for (i, inp) in node.inputs.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str(&inp.0.to_string());
            }
        }
    }
    s.push(')');
    s
}

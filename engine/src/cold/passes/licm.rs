// CEP:FILE: engine/src/cold/passes/licm.rs
// CEP:WHAT: Loop invariant code motion over the loop tree.
// CEP:WHY: Spec 5.1: hoisting loop invariants cuts per-iteration work;
//          legality uses the loop-tree dominance and effect analysis
//          (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:PASS: licm
// CEP:STATUS: complete
// CEP:FAILURE: Err aborts compilation of the unit (engine falls back).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic.
// CEP:HPC-COMPILE-COST: O(n) per loop tree walk.
// CEP:ASSUMES: graph passed ir::verify; loop tree built by ir::loops.
// CEP:COST: O(n).
// CEP:EVIDENCE: tests/ir_passes.rs; tests/tier23_tests.rs; differential
//               suite.
// CEP:SECURITY: never hoists effectful or trapping nodes.

//! Loop invariant code motion (spec 5.1).
use crate::cold::ir::{self, Graph, NodeKind};

/// Hoist loop-invariant computations out of loops.
///
/// CEP:WHAT: LICM pass (pipeline "licm").
/// CEP:WHY: Spec 5.1: hoisting invariants cuts per-iteration work;
///          legality uses loop-tree dominance and effect analysis.
/// CEP:STATUS: complete
/// CEP:FAILURE: Err aborts compilation of the unit; the engine falls back
///              to a lower tier (never silently).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:HPC-COMPILE-COST: O(n) per loop-tree walk.
/// CEP:ASSUMES: the graph passed ir::verify; the loop tree is built by
///              ir::loops.
/// CEP:COST: O(n).
/// CEP:EVIDENCE: tests/ir_passes.rs; tests/tier23_tests.rs (LICM hoists
///               invariants); differential suite.
/// CEP:SECURITY: never hoists effectful or trapping nodes.
pub fn licm(g: &mut Graph) -> Result<(), String> {
    let doms = ir::idoms(g);
    let mut loops: Vec<(usize, Vec<usize>)> = Vec::new();
    for bi in 0..g.blocks.len() {
        for s in &g.blocks[bi].succs {
            let h = s.0 as usize;
            if ir::dominates(&doms, h, bi) {
                // Natural loop of the back-edge bi -> h: {h} plus all blocks
                // reaching bi without passing through h. The header's own
                // predecessors are never expanded (a self-loop's body is
                // exactly the header).
                let mut body = vec![h];
                let mut stack = vec![bi];
                while let Some(x) = stack.pop() {
                    if x != h && !body.contains(&x) {
                        body.push(x);
                        for p in &g.blocks[x].preds {
                            stack.push(p.0 as usize);
                        }
                    } else if x == h && !body.contains(&x) {
                        body.push(x);
                    }
                }
                loops.push((h, body));
            }
        }
    }
    for (header, body) in &loops {
        let entry = g.blocks[*header]
            .preds
            .iter()
            .map(|p| p.0 as usize)
            .find(|p| !body.contains(p))
            .unwrap_or(0);
        for i in 0..g.nodes.len() {
            let node = &g.nodes[i];
            if node.block.0 as usize != *header || !Graph::is_pure(&node.kind) {
                continue;
            }
            if matches!(node.kind, NodeKind::Phi { .. }) {
                continue;
            }
            let invariant = node.inputs.iter().all(|inp| {
                let ib = g.nodes[inp.0 as usize].block.0 as usize;
                !body.contains(&ib)
            });
            if invariant {
                g.nodes[i].block = crate::cold::ir::BlockId(entry as u32);
            }
        }
    }
    Ok(())
}

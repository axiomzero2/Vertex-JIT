// CEP:FILE: engine/src/cold/passes/dead_code_elimination.rs
// CEP:WHAT: Dead code elimination over the effect/data graph.
// CEP:WHY: Spec 4.4: removing dead computations before GVN/LICM shrinks
//          the analyzed graph (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:PASS: dce
// CEP:STATUS: complete
// CEP:FAILURE: Err aborts compilation of the unit (engine falls back).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic.
// CEP:HPC-COMPILE-COST: O(n).
// CEP:ASSUMES: graph passed ir::verify; effectful nodes are never removed.
// CEP:COST: O(n).
// CEP:EVIDENCE: tests/ir_passes.rs; differential suite.
// CEP:SECURITY: effectful nodes and guards are never removed.

//! Dead code elimination (spec 4.4).
use crate::cold::ir::{DeoptSite, Graph, NodeId};

/// Remove nodes that no effect or live use reaches.
///
/// CEP:WHAT: Dead code elimination pass (pipeline "dce").
/// CEP:WHY: Spec 4.4: removing dead computations before GVN/LICM shrinks
///          the analyzed graph and exposes more folding.
/// CEP:STATUS: complete
/// CEP:FAILURE: Err aborts compilation of the unit; the engine falls back
///              to a lower tier (never silently).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:HPC-COMPILE-COST: O(n).
/// CEP:ASSUMES: the graph passed ir::verify; effectful nodes and guards
///              are never removed.
/// CEP:COST: O(n).
/// CEP:EVIDENCE: tests/ir_passes.rs; tests/differential.rs.
/// CEP:SECURITY: effectful nodes and guards are never removed.
pub fn dead_code_elimination(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>) -> Result<(), String> {
    let n = g.nodes.len();
    // Deopt sites anchor their referenced nodes: reconstruction reads the
    // recorded slots, so those values must survive.
    let mut anchored = vec![false; n];
    for (sn, s) in sites.iter() {
        if (sn.0 as usize) < n {
            anchored[sn.0 as usize] = true;
        }
        for (sn, _) in &s.stack {
            if (sn.0 as usize) < n {
                anchored[sn.0 as usize] = true;
            }
        }
        for (_, ln, _) in &s.locals {
            if (ln.0 as usize) < n {
                anchored[ln.0 as usize] = true;
            }
        }
    }
    let mut uses = vec![0u32; n];
    for node in &g.nodes {
        for inp in &node.inputs {
            uses[inp.0 as usize] += 1;
        }
    }
    let mut alive = vec![false; n];
    let mut changed = true;
    while changed {
        changed = false;
        for (i, node) in g.nodes.iter().enumerate() {
            if alive[i] {
                continue;
            }
            if !Graph::is_pure(&node.kind) || uses[i] > 0 || anchored[i] {
                alive[i] = true;
            }
        }
        for (i, node) in g.nodes.iter().enumerate() {
            if alive[i] {
                for inp in &node.inputs {
                    if !alive[inp.0 as usize] {
                        alive[inp.0 as usize] = true;
                        changed = true;
                    }
                }
            }
        }
    }
    let mut remap = vec![NodeId(u32::MAX); n];
    let mut next = 0u32;
    for (i, a) in alive.iter().enumerate() {
        if *a {
            remap[i] = NodeId(next);
            next += 1;
        }
    }
    // Rewrite deopt sites through the same remap; drop sites whose trigger
    // node was removed (they can never fire).
    sites.retain(|(sn, _)| (sn.0 as usize) < n && remap[sn.0 as usize].0 != u32::MAX);
    for (sn, site) in sites.iter_mut() {
        if (sn.0 as usize) < n && remap[sn.0 as usize].0 != u32::MAX {
            *sn = remap[sn.0 as usize];
        }
        for e in site.stack.iter_mut() {
            if (e.0 .0 as usize) < n && remap[e.0 .0 as usize].0 != u32::MAX {
                e.0 = remap[e.0 .0 as usize];
            }
        }
        for l in site.locals.iter_mut() {
            if (l.1 .0 as usize) < n && remap[l.1 .0 as usize].0 != u32::MAX {
                l.1 = remap[l.1 .0 as usize];
            }
        }
    }
    let mut new_nodes = Vec::with_capacity(next as usize);
    for (i, node) in g.nodes.iter().enumerate() {
        if !alive[i] {
            continue;
        }
        let mut nd = node.clone();
        for inp in &mut nd.inputs {
            *inp = remap[inp.0 as usize];
        }
        new_nodes.push(nd);
    }
    g.nodes = new_nodes;
    // Terminator, entry and block-head ids move with the compaction.
    g.start = remap[g.start.0 as usize];
    if let Some(r) = g.ret {
        g.ret = Some(remap[r.0 as usize]);
    }
    if let Some(h) = g.halt {
        g.halt = Some(remap[h.0 as usize]);
    }
    for b in &mut g.blocks {
        if let Some(h) = b.head {
            if remap[h.0 as usize].0 != u32::MAX {
                b.head = Some(remap[h.0 as usize]);
            } else {
                b.head = None;
            }
        }
    }
    Ok(())
}

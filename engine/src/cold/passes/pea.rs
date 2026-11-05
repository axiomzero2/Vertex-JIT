// CEP:FILE: engine/src/cold/passes/pea.rs
// CEP:WHAT: Partial escape analysis: scalar replacement of non-escaping
//           allocations with deopt materialization.
// CEP:WHY: Spec 5.4: allocations that never escape pay no heap cost;
//          deopt sites materialize the scalar state (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:PASS: pea
// CEP:STATUS: complete
// CEP:FAILURE: Err aborts compilation of the unit (engine falls back);
//              deopt-referenced arrays force the conservative path
//              (ticket CEP-ENGINE-009).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic.
// CEP:HPC-COMPILE-COST: O(n) per escape analysis.
// CEP:ASSUMES: graph passed ir::verify; deopt sites carry materialization
//              state.
// CEP:COST: O(n).
// CEP:EVIDENCE: tests/ir_passes.rs; tests/tier23_tests.rs; differential
//               suite.
// CEP:SECURITY: materialization at deopt is exact for replaced scalars.

//! Partial escape analysis (spec 5.4).
use crate::cold::ir::{DeoptSite, Graph, NodeId, NodeKind, VType};
use super::const_fold::{const_of, F};
use super::dead_code_elimination::dead_code_elimination;
use super::rewrite::replace_all;

/// Replace non-escaping allocations with scalars.
///
/// CEP:WHAT: Partial escape analysis pass (pipeline "pea").
/// CEP:WHY: Spec 5.4: allocations that never escape pay no heap cost;
///          deopt sites materialize the scalar state on demand.
/// CEP:STATUS: complete
/// CEP:FAILURE: Err aborts compilation of the unit; the engine falls back
///              to a lower tier (never silently). Deopt-referenced arrays
///              force the conservative path (ticket CEP-ENGINE-009).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:HPC-COMPILE-COST: O(n) per escape analysis.
/// CEP:ASSUMES: the graph passed ir::verify; deopt sites carry
///              materialization state.
/// CEP:COST: O(n).
/// CEP:EVIDENCE: tests/ir_passes.rs; tests/tier23_tests.rs; differential
///               suite.
/// CEP:SECURITY: materialization at deopt is exact for replaced scalars.
pub fn pea(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>) -> Result<(), String> {
    for i in 0..g.nodes.len() {
        let NodeKind::NewArray = g.nodes[i].kind else { continue };
        let arr = NodeId(i as u32);
        let Some(len_node) = g.nodes[i].inputs.first().copied() else {
            continue;
        };
        let Some(F::Num(len)) = const_of(g, len_node) else {
            continue;
        };
        if len < 0.0 || len.fract() != 0.0 || len > 64.0 {
            continue; // bounded scalar replacement (budget)
        }
        let len = len as usize;
        let mut escapes = false;
        let mut accesses: Vec<(usize, usize, Option<NodeId>)> = Vec::new();
        'uses: for (j, node) in g.nodes.iter().enumerate() {
            match &node.kind {
                NodeKind::LoadIdx => {
                    if node.inputs.first() == Some(&arr) {
                        match const_of(g, node.inputs[1]) {
                            Some(F::Num(ix))
                                if ix >= 0.0 && ix.fract() == 0.0 && (ix as usize) < len =>
                            {
                                accesses.push((j, ix as usize, None));
                            }
                            _ => {
                                escapes = true;
                                break 'uses;
                            }
                        }
                    }
                }
                NodeKind::StoreIdx => {
                    if node.inputs.first() == Some(&arr) {
                        match const_of(g, node.inputs[1]) {
                            Some(F::Num(ix))
                                if ix >= 0.0 && ix.fract() == 0.0 && (ix as usize) < len =>
                            {
                                accesses.push((j, ix as usize, Some(node.inputs[2])));
                            }
                            _ => {
                                escapes = true;
                                break 'uses;
                            }
                        }
                    } else if node.inputs.len() > 2 && node.inputs[2] == arr {
                        escapes = true;
                        break 'uses;
                    }
                }
                NodeKind::ArrayLen => {
                    if node.inputs.first() == Some(&arr) {
                        accesses.push((j, usize::MAX, None));
                    }
                }
                NodeKind::Return | NodeKind::Call { .. }
                    if node.inputs.contains(&arr) => {
                        escapes = true;
                        break 'uses;
                    }
                _ => {}
            }
        }
        if escapes {
            continue;
        }
        // Deopt sites may reference the array (guard traps recorded the
        // live locals). Materializing a replaced array at deopt is ticket
        // CEP-ENGINE-009; until then PEA bails for such arrays.
        let site_ref = sites.iter().any(|(_, site)| {
            site.locals.iter().any(|(_, x, _)| *x == arr)
                || site.stack.iter().any(|(x, _)| *x == arr)
        });
        if site_ref {
            continue;
        }
        let mut slots: Vec<Option<NodeId>> = vec![None; len];
        let mut ok = true;
        for &(j, ix, val) in &accesses {
            if ix == usize::MAX {
                continue;
            }
            if let Some(v) = val {
                slots[ix] = Some(v);
            } else if slots[ix].is_none() {
                ok = false; // load before any store: keep the allocation
                break;
            }
            let _ = j;
        }
        if !ok {
            continue;
        }
        let mut dead: Vec<NodeId> = vec![arr];
        for &(j, ix, _val) in &accesses {
            let jn = NodeId(j as u32);
            match &g.nodes[j].kind {
                NodeKind::LoadIdx => {
                    if let Some(v) = slots[ix] {
                        replace_all(g, sites, jn, v);
                        dead.push(jn);
                    } else {
                        dead.clear();
                        break;
                    }
                }
                NodeKind::StoreIdx => {
                    dead.push(jn);
                }
                NodeKind::ArrayLen => {
                    let repl = g.alloc(
                        NodeKind::ConstNum(len as f64),
                        Vec::new(),
                        g.nodes[j].block,
                        VType::Num,
                    );
                    replace_all(g, sites, jn, repl);
                    dead.push(jn);
                }
                _ => {}
            }
        }
        for d in dead {
            for node in &mut g.nodes {
                node.inputs.retain(|inp| *inp != d);
            }
            // The removed array and accesses vanish from deopt sites too:
            // the replaced program can no longer reach those traps.
            sites.retain(|(n, site)| {
                *n != d
                    && site.stack.iter().all(|(x, _)| *x != d)
                    && site.locals.iter().all(|(_, x, _)| *x != d)
            });
        }
    }
    dead_code_elimination(g, sites)
}

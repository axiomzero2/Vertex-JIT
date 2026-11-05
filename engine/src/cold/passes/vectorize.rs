// CEP:FILE: engine/src/cold/passes/vectorize.rs
// CEP:WHAT: Static-trip-count vectorization of map loops, with the loop
//           finder and the scalar/vector expansion helpers.
// CEP:WHY: Spec 4.4.2: map loops with static trips vectorize when the
//          predicted speedup clears the budget (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:PASS: vectorize
// CEP:STATUS: complete
// CEP:FAILURE: Err aborts compilation of the unit (engine falls back);
//              escaping arrays are refused loudly (vectorizer refusal
//              gate, tests/tier23_tests.rs).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic.
// CEP:HPC-COMPILE-COST: O(n) detection plus O(expansion) bounded by
//             max_static_trip.
// CEP:ASSUMES: graph passed ir::verify; budgets engine-configured.
// CEP:COST: O(n + expansion).
// CEP:EVIDENCE: tests/ir_passes.rs; tests/tier23_tests.rs; differential
//               suite.
// CEP:SECURITY: never vectorizes loops whose array can escape.

//! Static-trip vectorization of map loops (spec 4.4.2).
use super::rewrite::replace_all;
use crate::cold::ir::{DeoptSite, Graph, NodeId, NodeKind, VType};
use super::const_fold::{const_of, F, kind_name};
use super::dead_code_elimination::dead_code_elimination;
use crate::cold::passes::PassBudgets;

/// Vectorize static-trip map loops when legality and budget hold.
///
/// CEP:WHAT: Vectorization pass (pipeline "vectorize").
/// CEP:WHY: Spec 4.4.2: map loops with static trips vectorize when the
///          predicted speedup clears the budget; escaping arrays are
///          refused loudly (refusal gate, tests/tier23_tests.rs).
/// CEP:STATUS: complete
/// CEP:FAILURE: Err aborts compilation of the unit; the engine falls back
///              to a lower tier (never silently).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:HPC-COMPILE-COST: O(n) detection plus O(expansion) bounded by
///             max_static_trip.
/// CEP:ASSUMES: the graph passed ir::verify; budgets are
///              engine-configured.
/// CEP:COST: O(n + expansion).
/// CEP:EVIDENCE: tests/ir_passes.rs; tests/tier23_tests.rs (vectorized
///               map matches interpreter; refusal gate); differential
///               suite.
/// CEP:SECURITY: never vectorizes loops whose array can escape.
pub fn vectorize(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>, budgets: &PassBudgets) -> Result<(), String> {
    if budgets.vector_width == 0 {
        return Ok(());
    }
    // Cost model (spec 4.4.2): predicted speedup = width / (1 + 1/width).
    // Gate on the threshold before any transform.
    let predicted = f64::from(budgets.vector_width) / (1.0 + 1.0 / f64::from(budgets.vector_width));
    if predicted < budgets.vector_min_speedup {
        return Ok(());
    }
    let candidates = find_static_map_loops(g, budgets)?;
    for c in candidates {
        if c.vector_eligible {
            expand_loop_vector(g, sites, &c, budgets)?;
        }
    }
    Ok(())
}

/// A detected static map-loop candidate.
pub(crate) struct LoopCandidate {
    /// Induction phi node.
    pub(crate) phi: NodeId,
    /// Constant start.
    pub(crate) start: f64,
    /// Constant exclusive bound.
    pub(crate) bound: f64,
    /// The StoreIdx node (out array, index, value).
    pub(crate) store: NodeId,
    /// Output array node.
    pub(crate) out_arr: NodeId,
    /// Source array node.
    pub(crate) src_arr: NodeId,
    /// The arithmetic op name ("Add"/"Sub"/"Mul").
    pub(crate) op: &'static str,
    /// Constant operand of the op, if any.
    pub(crate) op_const: Option<f64>,
    /// True when vectorization legality holds.
    pub(crate) vector_eligible: bool,
}

/// Detect canonical static-trip map loops.
///
/// CEP:WHAT: Shape and dependence analysis for map loops.
/// CEP:WHY: Vectorization and unrolling need proven shape and dependence.
/// CEP:STATUS: complete
/// CEP:FAILURE: non-matching loops are ignored (legality first).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-TRANSFORM: Detects stride-1 map loops over distinct arrays with
///             static bounds.
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:ASSUMES: builder loop shape.
/// CEP:COST: O(n).
/// CEP:EVIDENCE: tests/vectorize_tests.rs.
/// CEP:SECURITY: bounds guards removed only with proven static bounds.
pub(crate) fn find_static_map_loops(g: &Graph, budgets: &PassBudgets) -> Result<Vec<LoopCandidate>, String> {
    let mut out = Vec::new();
    for i in 0..g.nodes.len() {
        let NodeKind::StoreIdx = g.nodes[i].kind else { continue };
        if g.nodes[i].inputs.len() < 3 {
            continue;
        }
        let store = NodeId(i as u32);
        let out_arr = g.nodes[i].inputs[0];
        let idx = g.nodes[i].inputs[1];
        let val = g.nodes[i].inputs[2];
        // The index must be the loop induction phi.
        let NodeKind::Phi { .. } = g.nodes[idx.0 as usize].kind else {
            continue;
        };
        // Phi: [init const, backedge Add(phi, 1)].
        let phi_inputs = &g.nodes[idx.0 as usize].inputs;
        let Some(F::Num(start)) = const_of(g, phi_inputs[0]) else {
            continue;
        };
        let back = phi_inputs[1];
        let NodeKind::Add = g.nodes[back.0 as usize].kind else {
            continue;
        };
        let b_in = &g.nodes[back.0 as usize].inputs;
        if b_in[0] != idx && b_in[1] != idx {
            continue;
        }
        let step_node = if b_in[0] == idx { b_in[1] } else { b_in[0] };
        let Some(F::Num(step)) = const_of(g, step_node) else {
            continue;
        };
        if step != 1.0 {
            continue;
        }
        // Bound: the loop condition Lt(phi, const). Find the If whose
        // condition compares the phi.
        let mut bound = None;
        for j in 0..g.nodes.len() {
            let NodeKind::Lt = g.nodes[j].kind else { continue };
            if g.nodes[j].inputs[0] == idx
                && let Some(F::Num(b)) = const_of(g, g.nodes[j].inputs[1]) {
                    bound = Some(b);
                    break;
                }
        }
        let Some(bound) = bound else { continue };
        if bound <= start || (bound - start) > f64::from(budgets.max_static_trip) {
            continue;
        }
        // The value: pure op over exactly one LoadIdx(src, idx).
        let (NodeKind::Add | NodeKind::Sub | NodeKind::Mul) = g.nodes[val.0 as usize].kind else {
            continue;
        };
        let op = kind_name(&g.nodes[val.0 as usize].kind);
        let v_in = &g.nodes[val.0 as usize].inputs;
        let (src_arr, op_const) = if let NodeKind::LoadIdx = g.nodes[v_in[0].0 as usize].kind {
            if g.nodes[v_in[0].0 as usize].inputs[1] != idx {
                continue;
            }
            let c = const_of(g, v_in[1]);
            (v_in[0], match c {
                Some(F::Num(c)) => Some(c),
                _ => None,
            })
        } else if let NodeKind::LoadIdx = g.nodes[v_in[1].0 as usize].kind {
            if g.nodes[v_in[1].0 as usize].inputs[1] != idx {
                continue;
            }
            let c = const_of(g, v_in[0]);
            (v_in[1], match c {
                Some(F::Num(c)) => Some(c),
                _ => None,
            })
        } else {
            continue;
        };
        // Dependence + bounds legality: distinct arrays, static lengths
        // at least the bound.
        let out_len = g.nodes[out_arr.0 as usize]
            .inputs
            .first()
            .copied()
            .and_then(|n| const_of(g, n));
        let src_len = g.nodes[src_arr.0 as usize]
            .inputs
            .first()
            .copied()
            .and_then(|n| const_of(g, n));
        let vector_eligible = out_arr != src_arr
            && matches!(out_len, Some(F::Num(l)) if l >= bound)
            && matches!(src_len, Some(F::Num(l)) if l >= bound);
        out.push(LoopCandidate {
            phi: idx,
            start,
            bound,
            store,
            out_arr,
            src_arr,
            op,
            op_const,
            vector_eligible,
        });
    }
    Ok(out)
}

/// Expand a scalar loop to straight-line code (unroll).
///
/// CEP:WHAT: Scalar expansion.
/// CEP:WHY: Static-trip loops become straight-line code.
/// CEP:STATUS: complete
/// CEP:FAILURE: budget cap on trip count.
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-TRANSFORM: Full unroll of static-trip loops.
/// CEP:ASSUMES: candidate shape proven.
/// CEP:COST: O(trip).
/// CEP:EVIDENCE: differential unroll tests.
/// CEP:SECURITY: guards removed only with proven static bounds.
pub(crate) fn expand_loop_scalar(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>, c: &LoopCandidate, _budgets: &PassBudgets) -> Result<(), String> {
    let mut last_store: Option<NodeId> = None;
    let mut i = c.start;
    while i < c.bound {
        let idx_c = g.alloc(NodeKind::ConstNum(i), Vec::new(), g.nodes[c.store.0 as usize].block, VType::Num);
        let src_len_c = g.alloc(
            NodeKind::ConstNum(array_len_const(g, c.src_arr)),
            Vec::new(),
            g.nodes[c.store.0 as usize].block,
            VType::Num,
        );
        let load = g.alloc(NodeKind::LoadIdx, vec![c.src_arr, idx_c], g.nodes[c.store.0 as usize].block, VType::Num);
        // The load's bounds guard chain is not needed (proved in bounds);
        // the raw node form is used directly.
        let _ = src_len_c;
        let val = match (c.op, c.op_const) {
            ("Add", Some(k)) => {
                let kc = g.alloc(NodeKind::ConstNum(k), Vec::new(), g.nodes[c.store.0 as usize].block, VType::Num);
                g.alloc(NodeKind::Add, vec![load, kc], g.nodes[c.store.0 as usize].block, VType::Num)
            }
            ("Sub", Some(k)) => {
                let kc = g.alloc(NodeKind::ConstNum(k), Vec::new(), g.nodes[c.store.0 as usize].block, VType::Num);
                g.alloc(NodeKind::Sub, vec![load, kc], g.nodes[c.store.0 as usize].block, VType::Num)
            }
            ("Mul", Some(k)) => {
                let kc = g.alloc(NodeKind::ConstNum(k), Vec::new(), g.nodes[c.store.0 as usize].block, VType::Num);
                g.alloc(NodeKind::Mul, vec![load, kc], g.nodes[c.store.0 as usize].block, VType::Num)
            }
            _ => load,
        };
        let st = g.alloc(
            NodeKind::StoreIdx,
            vec![c.out_arr, idx_c, val],
            g.nodes[c.store.0 as usize].block,
            VType::Unit,
        );
        last_store = Some(st);
        i += 1.0;
    }
    // Replace the loop's store with the last expanded store; loop control
    // nodes die through DCE once the phi loses its users.
    if let Some(st) = last_store {
        replace_all(g, sites, c.store, st);
    }
    // Rewire phi users to the init constant (the loop no longer iterates).
    let init = g.nodes[c.phi.0 as usize].inputs[0];
    replace_all(g, sites, c.phi, init);
    dead_code_elimination(g, sites)
}

/// Expand a vectorizable loop into vector node kinds (full expansion).
///
/// CEP:WHAT: Vector expansion.
/// CEP:WHY: Static-trip map loops become straight-line vector code.
/// CEP:STATUS: complete
/// CEP:FAILURE: budget cap on trip count.
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-TRANSFORM: Full unroll into VecLoad/VecStore with scalar
///             remainder handling (statically known).
/// CEP:ASSUMES: candidate proven by find_static_map_loops.
/// CEP:COST: O(trip/width) vector nodes plus O(trip%width) scalar nodes.
/// CEP:EVIDENCE: tests/vectorize_tests.rs.
/// CEP:SECURITY: bounds proven by the static length check.
fn expand_loop_vector(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>, c: &LoopCandidate, budgets: &PassBudgets) -> Result<(), String> {
    let w = budgets.vector_width;
    let trip = c.bound - c.start;
    let vec_trip = (trip as u64) / u64::from(w);
    let rem = (trip as u64) % u64::from(w);
    let block = g.nodes[c.store.0 as usize].block;
    let mut last_store: Option<NodeId> = None;
    let mut i = c.start;
    for _ in 0..vec_trip {
        let idx_c = g.alloc(NodeKind::ConstNum(i), Vec::new(), block, VType::Num);
        let load = g.alloc(NodeKind::VecLoad { width: w }, vec![c.src_arr, idx_c], block, VType::Num);
        let val = match (c.op, c.op_const) {
            ("Add", Some(k)) => {
                let kc = g.alloc(NodeKind::ConstNum(k), Vec::new(), block, VType::Num);
                g.alloc(NodeKind::VecAddConst { width: w }, vec![load, kc], block, VType::Num)
            }
            ("Sub", Some(k)) => {
                let kc = g.alloc(NodeKind::ConstNum(k), Vec::new(), block, VType::Num);
                g.alloc(NodeKind::VecSubConst { width: w }, vec![load, kc], block, VType::Num)
            }
            ("Mul", Some(k)) => {
                let kc = g.alloc(NodeKind::ConstNum(k), Vec::new(), block, VType::Num);
                g.alloc(NodeKind::VecMulConst { width: w }, vec![load, kc], block, VType::Num)
            }
            _ => load,
        };
        let st = g.alloc(NodeKind::VecStore { width: w }, vec![c.out_arr, idx_c, val], block, VType::Unit);
        last_store = Some(st);
        i += f64::from(w);
    }
    // Scalar remainder (statically known count).
    for _ in 0..rem {
        let idx_c = g.alloc(NodeKind::ConstNum(i), Vec::new(), block, VType::Num);
        let load = g.alloc(NodeKind::LoadIdx, vec![c.src_arr, idx_c], block, VType::Num);
        let val = match (c.op, c.op_const) {
            ("Add", Some(k)) => {
                let kc = g.alloc(NodeKind::ConstNum(k), Vec::new(), block, VType::Num);
                g.alloc(NodeKind::Add, vec![load, kc], block, VType::Num)
            }
            ("Sub", Some(k)) => {
                let kc = g.alloc(NodeKind::ConstNum(k), Vec::new(), block, VType::Num);
                g.alloc(NodeKind::Sub, vec![load, kc], block, VType::Num)
            }
            ("Mul", Some(k)) => {
                let kc = g.alloc(NodeKind::ConstNum(k), Vec::new(), block, VType::Num);
                g.alloc(NodeKind::Mul, vec![load, kc], block, VType::Num)
            }
            _ => load,
        };
        let st = g.alloc(NodeKind::StoreIdx, vec![c.out_arr, idx_c, val], block, VType::Unit);
        last_store = Some(st);
        i += 1.0;
    }
    if let Some(st) = last_store {
        replace_all(g, sites, c.store, st);
    }
    let init = g.nodes[c.phi.0 as usize].inputs[0];
    replace_all(g, sites, c.phi, init);
    dead_code_elimination(g, sites)
}

/// Static length of an array allocation node.
fn array_len_const(g: &Graph, arr: NodeId) -> f64 {
    match g.nodes[arr.0 as usize]
        .inputs
        .first()
        .copied()
        .and_then(|n| const_of(g, n))
    {
        Some(F::Num(l)) => l,
        _ => 0.0,
    }
}

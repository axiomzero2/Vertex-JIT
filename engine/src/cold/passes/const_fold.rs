// CEP:FILE: engine/src/cold/passes/const_fold.rs
// CEP:WHAT: Constant folding over proved-constant subgraphs (T1), with
//           the lattice helpers const_of and kind_name.
// CEP:WHY: Spec 4.1: folding happens before value numbering so GVN sees
//          canonical constants (doc/passes.md).
// CEP:CLASS: HPC-1
// CEP:PASS: const-fold
// CEP:STATUS: complete
// CEP:FAILURE: Err aborts compilation of the unit (engine falls back).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic.
// CEP:HPC-COMPILE-COST: O(n).
// CEP:ASSUMES: graph passed ir::verify.
// CEP:COST: O(n).
// CEP:EVIDENCE: tests/ir_passes.rs; differential suite.
// CEP:SECURITY: folds only proved constants; guards never weakened.

//! Constant folding over proved constants (spec 4.1).
use super::rewrite::replace_all;
use crate::cold::ir::{DeoptSite, Graph, NodeId, NodeKind, VType};
use super::dead_code_elimination::dead_code_elimination;

/// Fold proved-constant subgraphs to literals.
///
/// CEP:WHAT: Constant folding pass (pipeline "const-fold").
/// CEP:WHY: Spec 4.1: folding before value numbering lets GVN see
///          canonical constants and shrinks every later pass.
/// CEP:STATUS: complete
/// CEP:FAILURE: Err aborts compilation of the unit; the engine falls back
///              to a lower tier (never silently).
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:HPC-COMPILE-COST: O(n) over the node arena.
/// CEP:ASSUMES: the graph passed ir::verify; replacements funnel through
///              rewrite::replace_all and end with a DCE sweep.
/// CEP:COST: O(n).
/// CEP:EVIDENCE: tests/ir_passes.rs; tests/differential.rs (bit-identical
///               across tiers).
/// CEP:SECURITY: folds only proved constants; guards never weakened.
pub fn const_fold(g: &mut Graph, sites: &mut Vec<(NodeId, DeoptSite)>) -> Result<(), String> {
    for i in 0..g.nodes.len() {
        let kind = kind_name(&g.nodes[i].kind);
        if kind == "?" {
            continue;
        }
        let x = g.nodes[i].inputs[0];
        let y = g.nodes[i].inputs[1];
        let folded = match (const_of(g, x), const_of(g, y)) {
            (Some(F::Num(a)), Some(F::Num(b))) => match kind {
                "Add" => Some(F::Num(a + b)),
                "Sub" => Some(F::Num(a - b)),
                "Mul" => Some(F::Num(a * b)),
                "Div" if b != 0.0 => Some(F::Num(crate::hot::value::canonicalize_f64(a / b))),
                "Mod" => Some(F::Num(crate::hot::value::canonicalize_f64(a % b))),
                "Lt" => Some(F::Bool(a < b)),
                "Gt" => Some(F::Bool(a > b)),
                "Eq" => Some(F::Bool(a == b)),
                _ => None,
            },
            (Some(F::Bool(a)), Some(F::Bool(b))) => match kind {
                "Eq" => Some(F::Bool(a == b)),
                "Or" => Some(F::Bool(a || b)),
                _ => None,
            },
            _ => None,
        };
        if let Some(f) = folded {
            let repl = match f {
                F::Num(v) => g.alloc(NodeKind::ConstNum(v), Vec::new(), g.nodes[i].block, VType::Num),
                F::Bool(v) => g.alloc(
                    NodeKind::ConstBool(v),
                    Vec::new(),
                    g.nodes[i].block,
                    VType::Bool,
                ),
            };
            replace_all(g, sites, NodeId(i as u32), repl);
        }
    }
    dead_code_elimination(g, sites)
}

/// Value space of foldable constants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum F {
    Num(f64),
    Bool(bool),
}

/// Read a node's constant value.
pub(crate) fn const_of(g: &Graph, n: NodeId) -> Option<F> {
    match &g.nodes[n.0 as usize].kind {
        NodeKind::ConstNum(f) => Some(F::Num(*f)),
        NodeKind::ConstBool(b) => Some(F::Bool(*b)),
        _ => None,
    }
}

/// Stable kind name for dispatch tables.
pub(crate) fn kind_name(k: &NodeKind) -> &'static str {
    match k {
        NodeKind::Add => "Add",
        NodeKind::Sub => "Sub",
        NodeKind::Mul => "Mul",
        NodeKind::Div => "Div",
        NodeKind::Mod => "Mod",
        NodeKind::Lt => "Lt",
        NodeKind::Gt => "Gt",
        NodeKind::Eq => "Eq",
        NodeKind::Or => "Or",
        _ => "?",
    }
}

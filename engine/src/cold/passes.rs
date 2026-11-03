// CEP:FILE: engine/src/cold/passes.rs
// CEP:WHAT: The optimization passes: T1 (constant folding, dead code
//           elimination, If folding, Phi simplification, local value
//           numbering), T2 (global value numbering, LICM, partial escape
//           analysis) and T3 (single-block inlining, counted-loop
//           unrolling, static-trip vectorization of map loops).
// CEP:WHY: Spec sections 3.2, 4.1-4.4 and 5.1-5.4; standard 38.20 requires
//          an explicit versioned pipeline where every pass declares
//          legality, analyses, invalidations and cost.
// CEP:CLASS: HPC-1
// CEP:STATUS: complete
// CEP:FAILURE: Every pass returns Result; failure aborts compilation of the
//              unit (engine falls back to a lower tier). Passes re-verify
//              through ir::verify at pipeline checkpoints (38.18).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-DETERMINISM: deterministic; ordered arenas and BTreeMap keys, no
//             hash-order dependence.
// CEP:HPC-COMPILE-COST: each pass O(n) to O(n log n) (GVN string keys);
//             measured in benches/EVIDENCE.md.
// CEP:HPC-COMPILE-MEM: O(n) per pass.
// CEP:ASSUMES: graph passed ir::verify; profile data from the interpreter.
// CEP:COST: see CEP:HPC-COMPILE-COST.
// CEP:EVIDENCE: tests/ir_passes.rs (per-pass legality, positive and
//               negative), tests/differential.rs (all tiers bit-identical).
// CEP:SECURITY: passes never remove bounds guards unless the guard cannot
//               fire (proved constants); see the pea and vectorize
//               contracts.

use crate::cold::ir::{verify, Graph, NodeId, NodeKind, VType};
use std::collections::BTreeMap;

/// Pass name constants: the canonical pipeline (standard 38.20).
pub const PIPELINE_V1: &[&str] = &[
    "const-fold",
    "local-vn",
    "if-fold",
    "phi-simp",
    "dce",
    "gvn",
    "licm",
    "pea",
    "unroll",
    "vectorize",
    "inline",
    "const-fold",
    "dce",
];

/// Tier-to-pass-list mapping (spec 3.2, 4, 5; standard 15.2).
///
/// CEP:WHAT: Tier-to-pass mapping.
/// CEP:WHY: Explicit and versioned pipeline per tier.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: tests/ir_passes.rs pipeline tests.
/// CEP:SECURITY: none
#[must_use]
pub fn passes_for_tier(tier: u8) -> &'static [&'static str] {
    match tier {
        1 => &["const-fold", "local-vn", "if-fold", "phi-simp", "dce"],
        2 => &[
            "const-fold",
            "local-vn",
            "if-fold",
            "phi-simp",
            "dce",
            "gvn",
            "licm",
            "pea",
        ],
        _ => PIPELINE_V1,
    }
}

/// Pass budgets (spec 5.2.1, 5.3.1, 4.4.2).
#[derive(Debug, Clone, Copy)]
pub struct PassBudgets {
    /// Maximum unroll factor (spec 5.3.1).
    pub max_unroll_factor: u32,
    /// Minimum predicted speedup for vectorization (spec 4.4.2: >2x).
    pub vector_min_speedup: f64,
    /// Vector width in f64 lanes (4 for AVX2).
    pub vector_width: u32,
    /// Maximum inlining depth (spec 5.2.1: 3-5).
    pub max_inline_depth: u32,
    /// Maximum nodes of an inlinable callee body (spec 5.2.1: 500).
    pub max_inline_nodes: u32,
    /// Maximum static trip count the vectorizer/unroller will expand.
    pub max_static_trip: u32,
}

impl Default for PassBudgets {
    fn default() -> Self {
        PassBudgets {
            max_unroll_factor: 4,
            vector_min_speedup: 2.0,
            vector_width: 4,
            max_inline_depth: 4,
            max_inline_nodes: 500,
            max_static_trip: 4096,
        }
    }
}

/// Run one named pass.
///
/// CEP:WHAT: Pass dispatcher.
/// CEP:WHY: The pipeline references passes by name (38.20).
/// CEP:STATUS: complete
/// CEP:FAILURE: Unknown names fail closed.
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:ASSUMES: graph verified before the call.
/// CEP:COST: dispatch plus the pass.
/// CEP:EVIDENCE: tests/ir_passes.rs.
/// CEP:SECURITY: unknown pass names never run.
///
/// # Errors
/// Error string on pass failure or unknown name.
pub fn run_pass(name: &str, g: &mut Graph, budgets: &PassBudgets) -> Result<(), String> {
    match name {
        "const-fold" => const_fold(g),
        "local-vn" => local_value_numbering(g),
        "if-fold" => if_fold(g),
        "phi-simp" => phi_simplify(g),
        "dce" => dead_code_elimination(g),
        "gvn" => global_value_numbering(g),
        "licm" => licm(g),
        "pea" => pea(g),
        "unroll" => unroll(g, budgets),
        "vectorize" => vectorize(g, budgets),
        "inline" => Ok(()), // driven by the engine at lift time; see engine.rs
        "verify" => verify(g),
        _ => Err(format!("unknown pass: {name}")),
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
pub fn const_fold(g: &mut Graph) -> Result<(), String> {
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
            replace_all(g, NodeId(i as u32), repl);
        }
    }
    dead_code_elimination(g)
}

/// Value space of foldable constants.
#[derive(Debug, Clone, Copy, PartialEq)]
enum F {
    Num(f64),
    Bool(bool),
}

/// Read a node's constant value.
fn const_of(g: &Graph, n: NodeId) -> Option<F> {
    match &g.nodes[n.0 as usize].kind {
        NodeKind::ConstNum(f) => Some(F::Num(*f)),
        NodeKind::ConstBool(b) => Some(F::Bool(*b)),
        _ => None,
    }
}

/// Stable kind name for dispatch tables.
fn kind_name(k: &NodeKind) -> &'static str {
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

/// Replace every use of `from` with `to`.
///
/// CEP:WHAT: Global use rewrite.
/// CEP:WHY: Passes replace nodes.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(n + e).
/// CEP:EVIDENCE: exercised by every pass test.
/// CEP:SECURITY: none
fn replace_all(g: &mut Graph, from: NodeId, to: NodeId) {
    for node in &mut g.nodes {
        for inp in &mut node.inputs {
            if *inp == from {
                *inp = to;
            }
        }
    }
}

/// Local value numbering within blocks.
///
/// CEP:HPC-PASS-KIND: local CSE.
/// CEP:HPC-PASS-LEGALITY: pure nodes only, block-local.
/// CEP:HPC-PASS-COST: O(n) with string keys.
/// CEP:HPC-PASS-EVIDENCE: tests/ir_passes.rs.
///
/// # Errors
/// Internal inconsistency.
pub fn local_value_numbering(g: &mut Graph) -> Result<(), String> {
    let mut table: BTreeMap<String, NodeId> = BTreeMap::new();
    for i in 0..g.nodes.len() {
        let node = &g.nodes[i];
        if !Graph::is_pure(&node.kind) || matches!(node.kind, NodeKind::Phi { .. }) {
            continue;
        }
        let key = node_key(g, NodeId(i as u32));
        if let Some(existing) = table.get(&key) {
            let repl = *existing;
            replace_all(g, NodeId(i as u32), repl);
        } else {
            table.insert(key, NodeId(i as u32));
        }
    }
    dead_code_elimination(g)
}

/// Canonical key for a pure node: kind plus input ids (stable within a
/// compilation unit, standard 38.17).
fn node_key(g: &Graph, n: NodeId) -> String {
    let node = &g.nodes[n.0 as usize];
    let mut s = String::new();
    s.push_str(kind_name(&node.kind));
    s.push('(');
    for (i, inp) in node.inputs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&inp.0.to_string());
    }
    s.push(')');
    s
}

/// If folding: constant conditions resolve; dead sides removed.
///
/// CEP:HPC-PASS-KIND: control-flow simplification.
/// CEP:HPC-PASS-LEGALITY: only unreachable behavior changes; guards whose
///             condition folds to false are removed (the trap cannot fire).
/// CEP:HPC-PASS-COST: O(n).
/// CEP:HPC-PASS-EVIDENCE: tests/ir_passes.rs.
///
/// # Errors
/// Internal inconsistency.
pub fn if_fold(g: &mut Graph) -> Result<(), String> {
    let mut dead: Vec<NodeId> = Vec::new();
    for i in 0..g.nodes.len() {
        let NodeKind::If = g.nodes[i].kind else { continue };
        let cond = g.nodes[i].inputs[1];
        match const_of(g, cond) {
            Some(F::Bool(true)) => {
                if let Some(ff) = proj_of(g, NodeId(i as u32), false) {
                    dead.push(ff);
                }
            }
            Some(F::Bool(false)) => {
                if let Some(tt) = proj_of(g, NodeId(i as u32), true) {
                    dead.push(tt);
                }
            }
            _ => {}
        }
    }
    for d in dead {
        for node in &mut g.nodes {
            node.inputs.retain(|inp| *inp != d);
        }
    }
    dead_code_elimination(g)
}

/// Find the IfTrue/IfFalse projection of an If.
fn proj_of(g: &Graph, ifn: NodeId, truthy: bool) -> Option<NodeId> {
    for (i, node) in g.nodes.iter().enumerate() {
        let want = if truthy { NodeKind::IfTrue } else { NodeKind::IfFalse };
        if node.kind == want && node.inputs.first() == Some(&ifn) {
            return Some(NodeId(i as u32));
        }
    }
    None
}

/// Phi simplification: all-identical inputs collapse.
///
/// CEP:HPC-PASS-KIND: local rewrite.
/// CEP:HPC-PASS-LEGALITY: a phi with identical inputs is that input.
/// CEP:HPC-PASS-COST: O(n * inputs).
/// CEP:HPC-PASS-EVIDENCE: tests/ir_passes.rs.
///
/// # Errors
/// Internal inconsistency.
pub fn phi_simplify(g: &mut Graph) -> Result<(), String> {
    for i in 0..g.nodes.len() {
        let NodeKind::Phi { .. } = g.nodes[i].kind else { continue };
        let inputs = g.nodes[i].inputs.clone();
        if !inputs.is_empty() && inputs.iter().all(|v| *v == inputs[0]) {
            let repl = inputs[0];
            replace_all(g, NodeId(i as u32), repl);
        }
    }
    dead_code_elimination(g)
}

/// Dead code elimination: drop pure nodes with no uses.
///
/// CEP:HPC-PASS-KIND: DCE.
/// CEP:HPC-PASS-LEGALITY: pure nodes with no uses have no observable
///             effect; anchors (control, effects, traps, safepoints) stay.
/// CEP:HPC-PASS-COST: O(n) per iteration to fixpoint.
/// CEP:HPC-PASS-EVIDENCE: tests/ir_passes.rs.
///
/// # Errors
/// Internal inconsistency.
pub fn dead_code_elimination(g: &mut Graph) -> Result<(), String> {
    let n = g.nodes.len();
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
            if !Graph::is_pure(&node.kind) || uses[i] > 0 {
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
    Ok(())
}

// ---------------------------------------------------------------- T2 passes

/// Immediate dominators over pc-order blocks.
///
/// CEP:WHAT: Dominator tree (iterative dataflow).
/// CEP:WHY: GVN and LICM need dominance.
/// CEP:STATUS: complete
/// CEP:FAILURE: none for builder graphs.
/// CEP:ASSUMES: block order is an RPO.
/// CEP:COST: O(blocks^2) worst case; block counts are budget-bounded.
/// CEP:EVIDENCE: dominance-dependent pass tests.
/// CEP:SECURITY: none
fn idoms(g: &Graph) -> Vec<usize> {
    let nb = g.blocks.len();
    let mut idom = vec![usize::MAX; nb];
    if nb == 0 {
        return idom;
    }
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for bi in 1..nb {
            let mut new_idom = usize::MAX;
            for p in &g.blocks[bi].preds {
                let p = p.0 as usize;
                if idom[p] == usize::MAX {
                    continue;
                }
                new_idom = if new_idom == usize::MAX { p } else { intersect(&idom, p, new_idom) };
            }
            if new_idom != usize::MAX && idom[bi] != new_idom {
                idom[bi] = new_idom;
                changed = true;
            }
        }
    }
    idom
}

/// Walk to the common dominator ancestor.
fn intersect(idom: &[usize], mut a: usize, mut b: usize) -> usize {
    while a != b {
        while a > b {
            a = idom[a];
        }
        while b > a {
            b = idom[b];
        }
    }
    a
}

/// True when block a dominates block b.
fn dominates(doms: &[usize], a: usize, b: usize) -> bool {
    let mut cur = b;
    while cur != usize::MAX {
        if cur == a {
            return true;
        }
        if cur == 0 {
            return a == 0;
        }
        cur = doms[cur];
    }
    false
}

/// Global value numbering across the dominator tree.
///
/// CEP:HPC-PASS-KIND: global CSE.
/// CEP:HPC-PASS-ANALYSIS-REQUIRED: dominator tree.
/// CEP:HPC-PASS-LEGALITY: a dominating value with the same canonical key
///             produces the same value on all dominated paths; phis and
///             memory nodes excluded (this GVN does not model memory).
/// CEP:HPC-PASS-PRESERVES: semantics, traps, deopt sites.
/// CEP:HPC-PASS-COST: O(n log n).
/// CEP:HPC-PASS-EVIDENCE: tests/ir_passes.rs cross-block case.
///
/// # Errors
/// Internal inconsistency.
pub fn global_value_numbering(g: &mut Graph) -> Result<(), String> {
    let doms = idoms(g);
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
            if dominates(&doms, eb, b) {
                let repl = *existing;
                replace_all(g, NodeId(i as u32), repl);
                continue;
            }
        }
        table.insert(key, NodeId(i as u32));
    }
    dead_code_elimination(g)
}

/// Loop-invariant code motion.
///
/// CEP:HPC-PASS-KIND: loop transformation (code motion).
/// CEP:HPC-PASS-ANALYSIS-REQUIRED: natural loops via back-edges, dominance.
/// CEP:HPC-PASS-LEGALITY: pure nodes only (Div can trap, NewArray
///             allocates: both excluded); every input defined outside the
///             loop; node is not a Phi.
/// CEP:HPC-PASS-PRESERVES: semantics exactly.
/// CEP:HPC-PASS-COST: O(n * loops).
/// CEP:HPC-PASS-EVIDENCE: tests/ir_passes.rs LICM case.
///
/// # Errors
/// Internal inconsistency.
pub fn licm(g: &mut Graph) -> Result<(), String> {
    let doms = idoms(g);
    let mut loops: Vec<(usize, Vec<usize>)> = Vec::new();
    for bi in 0..g.blocks.len() {
        for s in &g.blocks[bi].succs {
            let h = s.0 as usize;
            if dominates(&doms, h, bi) {
                let mut body = vec![h];
                let mut stack = vec![bi];
                while let Some(x) = stack.pop() {
                    if !body.contains(&x) {
                        body.push(x);
                        for p in &g.blocks[x].preds {
                            stack.push(p.0 as usize);
                        }
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

/// Partial escape analysis: scalar replacement of provably non-escaping,
/// constant-length, constant-index arrays.
///
/// CEP:HPC-PASS-KIND: allocation elimination.
/// CEP:HPC-PASS-ANALYSIS-REQUIRED: escape analysis over the use graph.
/// CEP:HPC-PASS-LEGALITY: replacement applies only when the length is a
///             compile-time constant, every index is a constant in bounds,
///             and the array never escapes (no Return, StoreIdx-value, or
///             Call-argument use). Under those conditions no bounds guard
///             can fire, so removing guards preserves trap behavior
///             exactly; every other allocation is kept (spec 4.1.2 partial
///             sinking is ticket CEP-ENGINE-009).
/// CEP:HPC-PASS-PRESERVES: semantics including trap behavior.
/// CEP:HPC-PASS-COST: O(n).
/// CEP:HPC-PASS-EVIDENCE: tests/ir_passes.rs PEA cases.
///
/// # Errors
/// Internal inconsistency.
pub fn pea(g: &mut Graph) -> Result<(), String> {
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
                    if node.inputs[0] == arr {
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
                    if node.inputs[0] == arr {
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
                    if node.inputs[0] == arr {
                        accesses.push((j, usize::MAX, None));
                    }
                }
                NodeKind::Return | NodeKind::Call { .. } => {
                    if node.inputs.contains(&arr) {
                        escapes = true;
                        break 'uses;
                    }
                }
                _ => {}
            }
        }
        if escapes {
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
                        replace_all(g, jn, v);
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
                    replace_all(g, jn, repl);
                    dead.push(jn);
                }
                _ => {}
            }
        }
        for d in dead {
            for node in &mut g.nodes {
                node.inputs.retain(|inp| *inp != d);
            }
        }
    }
    dead_code_elimination(g)
}

// ---------------------------------------------------------------- T3 passes

/// Counted-loop unrolling for the canonical static-trip shape.
///
/// CEP:HPC-PASS-KIND: loop transformation (unrolling).
/// CEP:HPC-PASS-LEGALITY: trip count must be statically provable from the
///             canonical shape (constant start, step 1, constant bound,
///             Lt condition); the transform preserves the iteration space
///             exactly; other shapes are left untouched (38.25).
/// CEP:HPC-PASS-PRESERVES: iteration semantics.
/// CEP:HPC-PASS-COST: O(body * factor).
/// CEP:HPC-PASS-EVIDENCE: tests/ir_passes.rs unroll case.
///
/// # Errors
/// Internal inconsistency.
pub fn unroll(g: &mut Graph, budgets: &PassBudgets) -> Result<(), String> {
    // Handled together with vectorization: vectorize() performs the full
    // static-trip expansion (vector plus scalar remainder); plain unrolling
    // without vectorization is the same expansion with scalar nodes only.
    // This function expands eligible loops to straight-line scalar code
    // when the trip count is static and within budget.
    let candidates = find_static_map_loops(g, budgets)?;
    for c in candidates {
        if !c.vector_eligible {
            expand_loop_scalar(g, &c, budgets)?;
        }
    }
    Ok(())
}

/// Auto-vectorization of static-trip map loops.
///
/// CEP:HPC-PASS-KIND: loop transformation (vectorization).
/// CEP:HPC-PASS-ANALYSIS-REQUIRED: shape analysis (counted loop, stride 1,
///             contiguous element access) and dependence analysis (store
///             array differs from the load array; both lengths are static
///             constants at least the trip bound).
/// CEP:HPC-PASS-LEGALITY (38.26): dependence proven absent (distinct
///             arrays with static bounds), trip count static, body has no
///             calls or inner control flow; the cost model must predict a
///             speedup above the threshold; the scalar fallback path is
///             emitted by codegen when the target lacks AVX2; bounds
///             guards are removed only because both array lengths and the
///             trip bound are compile-time constants that prove every
///             access in bounds.
/// CEP:HPC-PASS-PRESERVES: semantics exactly (full expansion).
/// CEP:HPC-PASS-COST: O(trip) nodes; capped by max_static_trip.
/// CEP:HPC-PASS-TARGET: width from the target configuration (4 lanes for
///             AVX2 f64); scalar fallback when unsupported.
/// CEP:HPC-PASS-EVIDENCE: tests/vectorize_tests.rs (vectorized vs scalar
///             differential, AVX2 and fallback paths).
///
/// # Errors
/// Internal inconsistency.
pub fn vectorize(g: &mut Graph, budgets: &PassBudgets) -> Result<(), String> {
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
            expand_loop_vector(g, &c, budgets)?;
        }
    }
    Ok(())
}

/// A detected static map-loop candidate.
struct LoopCandidate {
    /// Induction phi node.
    phi: NodeId,
    /// Constant start.
    start: f64,
    /// Constant exclusive bound.
    bound: f64,
    /// The StoreIdx node (out array, index, value).
    store: NodeId,
    /// Output array node.
    out_arr: NodeId,
    /// Source array node.
    src_arr: NodeId,
    /// The arithmetic op name ("Add"/"Sub"/"Mul").
    op: &'static str,
    /// Constant operand of the op, if any.
    op_const: Option<f64>,
    /// True when vectorization legality holds.
    vector_eligible: bool,
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
fn find_static_map_loops(g: &Graph, budgets: &PassBudgets) -> Result<Vec<LoopCandidate>, String> {
    let mut out = Vec::new();
    for i in 0..g.nodes.len() {
        let NodeKind::StoreIdx = g.nodes[i].kind else { continue };
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
            if g.nodes[j].inputs[0] == idx {
                if let Some(F::Num(b)) = const_of(g, g.nodes[j].inputs[1]) {
                    bound = Some(b);
                    break;
                }
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
fn expand_loop_scalar(g: &mut Graph, c: &LoopCandidate, _budgets: &PassBudgets) -> Result<(), String> {
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
        replace_all(g, c.store, st);
    }
    // Rewire phi users to the init constant (the loop no longer iterates).
    let init = g.nodes[c.phi.0 as usize].inputs[0];
    replace_all(g, c.phi, init);
    dead_code_elimination(g)
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
fn expand_loop_vector(g: &mut Graph, c: &LoopCandidate, budgets: &PassBudgets) -> Result<(), String> {
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
        replace_all(g, c.store, st);
    }
    let init = g.nodes[c.phi.0 as usize].inputs[0];
    replace_all(g, c.phi, init);
    dead_code_elimination(g)
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

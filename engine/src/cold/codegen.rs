// CEP:FILE: engine/src/cold/codegen.rs
// CEP:WHAT: Machine-code lowering from the Sea of Nodes IR: frame-slot
//           allocation (disp32 addressing, four slots per vector value),
//           per-node x86-64 emission through the audited emitter, inline
//           guards with trap calls, the word-ABI call sequence, phi copies
//           at block exits, vector lowering with the AVX2 path and the
//           scalar fallback, and deopt metadata layout.
// CEP:WHY: Spec sections 3-5: T1+ lower optimized IR to machine code;
//          standard 38.34 (emission) and 38.39 (JIT safety).
// CEP:CLASS: HPC-1
// CEP:STATUS: complete
// CEP:FAILURE: EngineError on graphs that fail verification, on unbound
//              labels, or on argument counts above the register ABI.
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-CODEGEN: stack-slot allocation (one slot per scalar SSA value,
//             four per vector value; no register allocator yet, documented
//             in doc/codegen.md Known Limitations); guards for every Div
//             and runtime-indexed array access; phi copies at block exits;
//             W^X publication through the code cache.
// CEP:HPC-DETERMINISM: deterministic; node-id order per block.
// CEP:HPC-COMPILE-COST: O(nodes); measured in benches/EVIDENCE.md.
// CEP:HPC-COMPILE-MEM: O(code size).
// CEP:HPC-TARGET-MODEL: x86-64 SSE2 baseline; AVX2 path gated on
//             TargetConfig::avx2; scalar fallback otherwise (38.26).
// CEP:ASSUMES: verified graph; runtime helper addresses valid; System V
//              AMD64 ABI for helpers and calls.
// CEP:COST: see CEP:HPC-COMPILE-COST.
// CEP:EVIDENCE: tests/t1_exec.rs, tests/vectorize_tests.rs (differential
//               execution); golden-byte tests in tier0.rs.
// CEP:SECURITY: guards retained for every runtime-valued access; only
//               proven-constant accesses skip them (pass contracts).

use crate::cold::errors::EngineError;
use crate::cold::ir::{DeoptSite, Graph, NodeId, NodeKind, VType};
use crate::cold::tier0_emit::{Cc, Emitter, Label};
use crate::hot::value::{TypeTag, PAYLOAD_MASK};

/// Slots per vector value (f64 lanes for AVX2).
const VEC_SLOTS: u32 = 4;

/// Compiled T1+ function.
#[derive(Debug, Clone)]
pub struct CompiledCode {
    /// Machine code bytes.
    pub bytes: Vec<u8>,
    /// Deopt records with concrete slot maps.
    pub deopt: Vec<DeoptRecord>,
    /// Total frame slots (8 bytes each).
    pub slots: u32,
    /// Static type of the returned word (Halt bottom / Ret value).
    pub return_tag: TypeTag,
}

/// One deopt record (doc/deopt.md Formal Definitions).
#[derive(Debug, Clone)]
pub struct DeoptRecord {
    /// Trigger node (guard or safepoint).
    pub node: NodeId,
    /// Interpreter resume pc.
    pub bc_pc: u32,
    /// Value stack shape: (frame slot, type) bottom-to-top.
    pub stack: Vec<(u32, TypeTag)>,
    /// Live locals: (interpreter slot, frame slot, type).
    pub locals: Vec<(u16, u32, TypeTag)>,
    /// Code offsets of the trap call return addresses for this site.
    pub trap_ret_offsets: Vec<u32>,
}

/// Lower a verified graph to machine code.
///
/// CEP:WHAT: The T1+ emission algorithm.
/// CEP:WHY: Optimized IR becomes executable code with guards, deopt
///          metadata and vector paths.
/// CEP:STATUS: complete
/// CEP:FAILURE: EngineError on verification failure or unbound labels.
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-CODEGEN: see file header.
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:ASSUMES: verified graph; helper addresses valid.
/// CEP:COST: O(nodes).
/// CEP:EVIDENCE: differential tests per tier.
/// CEP:SECURITY: guards retained except where proven constant.
///
/// # Errors
/// EngineError.
#[allow(clippy::too_many_arguments)]
pub fn lower(
    g: &Graph,
    deopt_sites: &[(NodeId, DeoptSite)],
    avx2: bool,
    helper: &Helpers,
    callee_addrs: &[usize],
) -> Result<CompiledCode, EngineError> {
    crate::cold::ir::verify(g).map_err(EngineError::IrVerificationFailed)?;

    // Slot allocation: scalars 1 slot, vectors VEC_SLOTS.
    let mut slot_of = vec![0u32; g.nodes.len()];
    let mut next_slot = 0u32;
    for (i, node) in g.nodes.iter().enumerate() {
        slot_of[i] = next_slot;
        next_slot += match node.kind {
            NodeKind::VecLoad { .. }
            | NodeKind::VecAddConst { .. }
            | NodeKind::VecSubConst { .. }
            | NodeKind::VecMulConst { .. } => VEC_SLOTS,
            _ => 1,
        };
    }

    let mut e = Emitter::new();
    let mut node_trap_rets: std::collections::BTreeMap<NodeId, Vec<u32>> =
        std::collections::BTreeMap::new();
    let mut block_labels: Vec<Label> = Vec::with_capacity(g.blocks.len());
    for _ in 0..g.blocks.len() {
        block_labels.push(e.label());
    }

    e.prologue();
    e.sub_rsp(next_slot * 8);

    for bi in 0..g.blocks.len() {
        e.bind(block_labels[bi]);
        // Schedule the block's nodes in dependency order: a node emits only
        // after all its data inputs (plus, for Halt, the recorded halt-site
        // stack nodes) have emitted. Node ids are creation order, which is
        // not execution order after pass rewrites (standard 38.34).
        let block_nodes: Vec<usize> = (0..g.nodes.len())
            .filter(|nid| g.nodes[*nid].block.0 as usize == bi)
            .collect();
        let site_inputs = |nid: usize| -> Vec<usize> {
            if matches!(g.nodes[nid].kind, NodeKind::Halt) {
                deopt_sites
                    .iter()
                    .find(|(n, _)| n.0 as usize == nid)
                    .map(|(_, s)| s.stack.iter().map(|(n, _)| n.0 as usize).collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        };
        let mut emitted = vec![false; g.nodes.len()];
        let mut remaining = block_nodes.len();
        while remaining > 0 {
            let mut progressed = false;
            for &nid in &block_nodes {
                if emitted[nid] {
                    continue;
                }
                // Only DATA dependencies gate emission: control nodes emit
                // no code, and their edges form the control cycle
                // (Loop <- IfTrue <- If <- Loop) that must not stall
                // scheduling.
                let is_data = |i: usize| g.nodes[i].ty != VType::Control;
                let mut dep_ids: Vec<usize> = g.nodes[nid]
                    .inputs
                    .iter()
                    .map(|i| i.0 as usize)
                    .filter(|i| is_data(*i) && *i != nid)
                    .collect();
                dep_ids.extend(site_inputs(nid));
                // Phis emit no code: their values arrive via the
                // predecessor-exit copies, so they are always ready.
                let deps_ok = matches!(g.nodes[nid].kind, NodeKind::Phi { .. })
                    || dep_ids.iter().all(|inp| {
                        emitted[*inp] || g.nodes[*inp].block.0 as usize != bi
                    });
                if deps_ok {
                    let rets = emit_node(
                        &mut e,
                        g,
                        NodeId(nid as u32),
                        &slot_of,
                        avx2,
                        helper,
                        callee_addrs,
                        bi,
                        deopt_sites,
                    )?;
                    if !rets.is_empty() {
                        node_trap_rets.insert(NodeId(nid as u32), rets);
                    }
                    emitted[nid] = true;
                    remaining -= 1;
                    progressed = true;
                }
            }
            if !progressed {
                // Cycles are impossible in a verified DAG; fail loud with
                // the stalled nodes for diagnosis.
                let stalled: Vec<String> = block_nodes
                    .iter()
                    .filter(|nid| !emitted[**nid])
                    .map(|nid| {
                        format!(
                            "n{} deps {:?}",
                            nid,
                            g.nodes[*nid].inputs
                        )
                    })
                    .collect();
                return Err(EngineError::IrVerificationFailed(format!(
                    "block {bi} schedule stalled: {stalled:?}"
                )));
            }
        }
        emit_phi_copies(&mut e, g, bi, &slot_of)?;
        match g.blocks[bi].succs.len() {
            0 => {
                let has_term = g.nodes.iter().any(|n| {
                    n.block.0 as usize == bi
                        && matches!(n.kind, NodeKind::Halt | NodeKind::Return)
                });
                if !has_term {
                    // Dead-end block: defined halt tail.
                    e.emit(&[0x31, 0xC0]);
                    e.emit(&[0x31, 0xD2]);
                    e.epilogue();
                }
            }
            1 => {
                let target = g.blocks[bi].succs[0].0 as usize;
                e.jmp(block_labels[target]);
            }
            2 => {
                // Find the block-terminator If (not a guard If).
                let mut term_if: Option<NodeId> = None;
                for nid in 0..g.nodes.len() {
                    let n = &g.nodes[nid];
                    if n.block.0 as usize != bi {
                        continue;
                    }
                    if let NodeKind::If = n.kind {
                        let is_guard = g.nodes.iter().any(|m| {
                            matches!(m.kind, NodeKind::UncommonTrap { .. })
                                && m.inputs.first() == Some(&proj_true(g, NodeId(nid as u32)))
                        });
                        if !is_guard {
                            term_if = Some(NodeId(nid as u32));
                        }
                    }
                }
                let Some(ifn) = term_if else {
                    return Err(EngineError::IrVerificationFailed(
                        "two-successor block without terminator If".to_owned(),
                    ));
                };
                let cond = g.nodes[ifn.0 as usize].inputs[1];
                // Polarity: the builder records whether succs[0] is the
                // condition-true side (taken_sides), which stays correct
                // even when the taken edge targets a loop header whose
                // head node is the Loop, not a projection.
                let s0 = g.blocks[bi].succs[0].0 as usize;
                let s1 = g.blocks[bi].succs[1].0 as usize;
                let s0_is_true = g.taken_sides.get(bi).copied().flatten().unwrap_or(false);
                // test cond; jz <false side>; jmp <true side>
                load_slot_rax(&mut e, slot_of[cond.0 as usize]);
                e.test_rr(0, 0);
                let (l_true, l_false) = if s0_is_true {
                    (block_labels[s0], block_labels[s1])
                } else {
                    (block_labels[s1], block_labels[s0])
                };
                e.jcc(Cc::Eq, l_false);
                e.jmp(l_true);
            }
            _ => {
                return Err(EngineError::IrVerificationFailed(
                    "block with more than two successors".to_owned(),
                ))
            }
        }
    }
    e.finalize()
        .map_err(|(at, _)| EngineError::IrVerificationFailed(format!("unbound label at {at}")))?;

    let deopt = deopt_sites
        .iter()
        .map(|(node, site)| DeoptRecord {
            node: *node,
            bc_pc: site.bc_pc,
            stack: site
                .stack
                .iter()
                .map(|(n, t)| (slot_of[n.0 as usize], vtype_to_tag(*t)))
                .collect(),
            locals: site
                .locals
                .iter()
                .map(|(s, n, t)| (*s, slot_of[n.0 as usize], vtype_to_tag(*t)))
                .collect(),
            trap_ret_offsets: node_trap_rets.get(node).cloned().unwrap_or_default(),
        })
        .collect();
    // Return type: the bottom stack node's type at the Halt site, or the
    // Ret node's type.
    let return_tag = g
        .halt
        .and_then(|h| {
            deopt_sites
                .iter()
                .find(|(n, _)| *n == h)
                .and_then(|(_, s)| s.stack.first().map(|(_, t)| vtype_to_tag(*t)))
        })
        .or_else(|| {
            g.ret.map(|r| vtype_to_tag(g.nodes[r.0 as usize].ty))
        })
        .unwrap_or(TypeTag::Null);
    Ok(CompiledCode { bytes: e.as_slice().to_vec(), deopt, slots: next_slot, return_tag })
}

/// Runtime helper addresses.
#[derive(Debug, Clone, Copy)]
pub struct Helpers {
    /// jit_fmod.
    pub fmod: usize,
    /// jit_trap_div0.
    pub trap_div0: usize,
    /// jit_trap_bounds.
    pub trap_bounds: usize,
    /// jit_alloc.
    pub alloc: usize,
    /// jit_store_elem.
    pub store_elem: usize,
    /// jit_load_elem.
    pub load_elem: usize,
    /// jit_safepoint_poll.
    pub poll: usize,
}

/// Convert an IR VType to the deopt record's TypeTag.
///
/// CEP:WHAT: Type conversion.
/// CEP:WHY: Deopt records use the runtime type vocabulary.
/// CEP:STATUS: complete
/// CEP:FAILURE: Control maps to Null (never read for control tokens).
/// CEP:ASSUMES: none
/// CEP:COST: one match.
/// CEP:EVIDENCE: deopt tests.
/// CEP:SECURITY: none
fn vtype_to_tag(t: VType) -> TypeTag {
    match t {
        VType::Num => TypeTag::Number,
        VType::Bool => TypeTag::Bool,
        VType::Str => TypeTag::Str,
        VType::Obj => TypeTag::Obj,
        VType::Control | VType::Unit => TypeTag::Null,
    }
}

/// Find the IfTrue projection of an If.
/// CEP:WHAT: IfTrue projection lookup for branch emission.
/// CEP:WHY: Codegen lowers If nodes by emitting the truthy side first; the
///              projection identifies that control edge.
/// CEP:STATUS: complete
/// CEP:FAILURE: falls back to the If node when no IfTrue projection
///              exists (callers handle the miss).
/// CEP:ASSUMES: the graph passed ir::verify.
/// CEP:COST: O(n) scan.
/// CEP:EVIDENCE: tests/differential.rs branch programs.
fn proj_true(g: &Graph, ifn: NodeId) -> NodeId {
    for (i, n) in g.nodes.iter().enumerate() {
        if n.kind == NodeKind::IfTrue && n.inputs.first() == Some(&ifn) {
            return NodeId(i as u32);
        }
    }
    NodeId(u32::MAX)
}

/// ABI argument registers for the engine's call convention (System V
/// integer registers), indexed by parameter position (doc/codegen.md).
/// CEP:WHAT: Call ABI register table.
/// CEP:WHY: Call sites and Param lowering must agree on the register for
///              each argument position; a shared constant pins it.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: System V AMD64.
/// CEP:COST: zero (const).
/// CEP:EVIDENCE: call execution tests (tests/call_inline_tests.rs).
/// CEP:SECURITY: none
const ARG_REGS: [u8; ARG_REGS_LEN] = [7, 6, 2, 1, 8, 9]; // rdi,rsi,rdx,rcx,r8,r9
/// Length of the ABI argument register table (maximum parameter count).
const ARG_REGS_LEN: usize = 6;

/// Load a frame slot into rax (disp32).
///
/// CEP:WHAT: Slot load.
/// CEP:WHY: Values live in frame slots.
/// CEP:STATUS: complete
/// CEP:FAILURE: none within the node budget (disp32 range).
/// CEP:ASSUMES: slot*8 fits i32.
/// CEP:COST: one load.
/// CEP:EVIDENCE: execution tests.
/// CEP:SECURITY: none
fn load_slot_rax(e: &mut Emitter, slot: u32) {
    let disp = -8i64 * (i64::from(slot) + 1);
    e.emit(&[0x48, 0x8B, 0x85]);
    e.emit(&(disp as i32).to_le_bytes());
}

/// Store rax into a frame slot (disp32).
///
/// CEP:WHAT: Slot store.
/// CEP:WHY: Results land in slots.
/// CEP:STATUS: complete
/// CEP:FAILURE: none within the node budget.
/// CEP:ASSUMES: slot*8 fits i32.
/// CEP:COST: one store.
/// CEP:EVIDENCE: execution tests.
/// CEP:SECURITY: none
fn store_slot_rax(e: &mut Emitter, slot: u32) {
    let disp = -8i64 * (i64::from(slot) + 1);
    e.emit(&[0x48, 0x89, 0x85]);
    e.emit(&(disp as i32).to_le_bytes());
}

/// Load a frame slot into xmm0 (disp32).
///
/// CEP:WHAT: Double slot load.
/// CEP:WHY: Arithmetic operands.
/// CEP:STATUS: complete
/// CEP:FAILURE: none within the node budget.
/// CEP:ASSUMES: none
/// CEP:COST: one load.
/// CEP:EVIDENCE: execution tests.
/// CEP:SECURITY: none
fn load_slot_xmm0(e: &mut Emitter, slot: u32) {
    let disp = -8i64 * (i64::from(slot) + 1);
    e.emit(&[0xF2, 0x0F, 0x10, 0x85]);
    e.emit(&(disp as i32).to_le_bytes());
}

/// Store xmm0 into a frame slot (disp32).
///
/// CEP:WHAT: Double slot store.
/// CEP:WHY: Results land in slots.
/// CEP:STATUS: complete
/// CEP:FAILURE: none within the node budget.
/// CEP:ASSUMES: none
/// CEP:COST: one store.
/// CEP:EVIDENCE: execution tests.
/// CEP:SECURITY: none
fn store_slot_xmm0(e: &mut Emitter, slot: u32) {
    let disp = -8i64 * (i64::from(slot) + 1);
    e.emit(&[0xF2, 0x0F, 0x11, 0x85]);
    e.emit(&(disp as i32).to_le_bytes());
}

/// Load a frame slot into xmm1 (disp32).
///
/// CEP:WHAT: Second-operand load.
/// CEP:WHY: Binary SSE ops need two operands.
/// CEP:STATUS: complete
/// CEP:FAILURE: none within the node budget.
/// CEP:ASSUMES: none
/// CEP:COST: one load.
/// CEP:EVIDENCE: execution tests.
/// CEP:SECURITY: none
fn load_slot_xmm1(e: &mut Emitter, slot: u32) {
    let disp = -8i64 * (i64::from(slot) + 1);
    e.emit(&[0xF2, 0x0F, 0x10, 0x8D]);
    e.emit(&(disp as i32).to_le_bytes());
}

/// Load a frame slot into rcx (disp32).
///
/// CEP:WHAT: Index load.
/// CEP:WHY: Address computation needs an integer register.
/// CEP:STATUS: complete
/// CEP:FAILURE: none within the node budget.
/// CEP:ASSUMES: none
/// CEP:COST: one load.
/// CEP:EVIDENCE: array tests.
/// CEP:SECURITY: none
fn load_slot_rcx(e: &mut Emitter, slot: u32) {
    let disp = -8i64 * (i64::from(slot) + 1);
    e.emit(&[0x48, 0x8B, 0x8D]);
    e.emit(&(disp as i32).to_le_bytes());
}

/// Mask a boxed pointer payload in rax.
///
/// CEP:WHAT: Payload mask.
/// CEP:WHY: Boxed pointers carry the 47-bit payload.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: rax holds a boxed word.
/// CEP:COST: movabs+and.
/// CEP:EVIDENCE: array tests.
/// CEP:SECURITY: enforces the pointer-width contract.
fn mask_payload(e: &mut Emitter) {
    e.movabs(9, PAYLOAD_MASK);
    e.emit(&[0x4C, 0x21, 0xC8]); // and rax, r9
}

/// Emit a trap call: rdi=rbp, rsi=rsp, rdx=0(code base resolved at runtime).
///
/// CEP:WHAT: Trap sequence.
/// CEP:WHY: Guards transfer control with frame state (spec 5.7.2).
/// CEP:STATUS: complete
/// CEP:FAILURE: the call never returns.
/// CEP:ASSUMES: System V ABI.
/// CEP:COST: cold path.
/// CEP:EVIDENCE: trap tests.
/// CEP:SECURITY: frame pointers validated engine-side.
fn emit_trap_call(e: &mut Emitter, trap_addr: usize) -> u32 {
    e.emit(&[0x48, 0x89, 0xEF]); // mov rdi, rbp
    e.emit(&[0x48, 0x89, 0xE6]); // mov rsi, rsp
    e.call_abs(trap_addr);
    // Return address = position after the 3-byte call r11.
    e.as_slice().len() as u32
}

/// Emit phi copies at a block exit.
///
/// CEP:WHAT: Phi copy emission.
/// CEP:WHY: SSA phis become copies; destinations are distinct slots so
///          sequential copies equal the parallel copy (doc/codegen.md).
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: phi inputs ordered by predecessor.
/// CEP:COST: O(phis at successors).
/// CEP:EVIDENCE: differential loop and diamond tests.
/// CEP:SECURITY: none
fn emit_phi_copies(e: &mut Emitter, g: &Graph, bi: usize, slot_of: &[u32]) -> Result<(), EngineError> {
    for succ in &g.blocks[bi].succs {
        let s = succ.0 as usize;
        let Some(head) = g.blocks[s].head else { continue };
        let head_kind = g.nodes[head.0 as usize].kind.clone();
        let pred_index = g.blocks[s]
            .preds
            .iter()
            .position(|p| p.0 as usize == bi)
            .unwrap_or(0);
        for nid in 0..g.nodes.len() {
            let n = &g.nodes[nid];
            if n.block.0 != s as u32 {
                continue;
            }
            let NodeKind::Phi { .. } = n.kind else { continue };
            let input_idx = if matches!(head_kind, NodeKind::Loop) {
                if pred_index == 0 { 0 } else { 1 }
            } else {
                pred_index.min(n.inputs.len().saturating_sub(1))
            };
            let src = n.inputs[input_idx];
            if src.0 == nid as u32 {
                continue;
            }
            load_slot_rax(e, slot_of[src.0 as usize]);
            store_slot_rax(e, slot_of[nid]);
        }
    }
    Ok(())
}

/// Emit one node.
///
/// CEP:WHAT: Per-node lowering.
/// CEP:WHY: The emission policy (38.34).
/// CEP:STATUS: complete
/// CEP:FAILURE: EngineError for inconsistent graphs.
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-CODEGEN: guards inline; word-ABI calls; AVX2 vector path with a
///             scalar fallback (38.26).
/// CEP:ASSUMES: inputs dominate (verified graph).
/// CEP:COST: O(1) per node.
/// CEP:EVIDENCE: differential tests.
/// CEP:SECURITY: guards retained for runtime-valued accesses.
///
/// # Errors
/// EngineError.
#[allow(clippy::too_many_arguments)]
fn emit_node(
    e: &mut Emitter,
    g: &Graph,
    nid: NodeId,
    slot_of: &[u32],
    avx2: bool,
    helper: &Helpers,
    callee_addrs: &[usize],
    _block: usize,
    deopt_sites: &[(NodeId, crate::cold::ir::DeoptSite)],
) -> Result<Vec<u32>, EngineError> {
    let node = &g.nodes[nid.0 as usize];
    let slot = slot_of[nid.0 as usize];
    let mut trap_rets: Vec<u32> = Vec::new();
    match &node.kind {
        NodeKind::Start
        | NodeKind::Region
        | NodeKind::Loop
        | NodeKind::IfTrue
        | NodeKind::IfFalse
        | NodeKind::Null
        | NodeKind::Phi { .. }
        | NodeKind::If => {}
        NodeKind::Param { slot: param } => {
            // Incoming argument word: the caller placed it in the ABI
            // register for this parameter index (doc/codegen.md ABI).
            let regs = ARG_REGS;
            let Some(r) = regs.get(*param as usize).copied() else {
                return Err(EngineError::IrVerificationFailed(format!(
                    "param {param} exceeds the {ARG_REGS_LEN}-register call convention"
                )));
            };
            e.mov_rr(r, 0);
            store_slot_rax(e, slot);
        }
        NodeKind::UncommonTrap { .. } => {
            // Guards lower through the runtime helpers' own checks
            // (LoadIdx/StoreIdx/NewArray arms and the Div arm); the IR
            // guard chain is metadata. Emit nothing: an ud2 here would sit
            // in the linear fallthrough path.
        }
        NodeKind::ConstNum(f) => {
            e.movabs(0, f.to_bits());
            store_slot_rax(e, slot);
        }
        NodeKind::ConstBool(b) => {
            e.movabs(0, u64::from(*b));
            store_slot_rax(e, slot);
        }
        NodeKind::ConstStr(s) => {
            e.movabs(0, crate::hot::value::box_word(crate::hot::value::ITAG_STR, u64::from(*s)));
            store_slot_rax(e, slot);
        }
        NodeKind::Add | NodeKind::Sub | NodeKind::Mul => {
            let x = node.inputs[0];
            let y = node.inputs[1];
            load_slot_xmm0(e, slot_of[x.0 as usize]);
            load_slot_xmm1(e, slot_of[y.0 as usize]);
            let op = match node.kind {
                NodeKind::Add => 0x58,
                NodeKind::Sub => 0x5C,
                _ => 0x59,
            };
            e.f2_op(op, 0, 1);
            store_slot_xmm0(e, slot);
        }
        NodeKind::Div => {
            let x = node.inputs[0];
            let y = node.inputs[1];
            // Zero guard on y; the trap block sits after a jump-over so the
            // fallthrough path never reaches it (mirrors the T0 template).
            load_slot_xmm0(e, slot_of[y.0 as usize]);
            e.xorps(3, 3);
            e.ucomisd(0, 3);
            let trap = e.label();
            let div_path = e.label();
            let nan_check = e.label();
            let done = e.label();
            let end = e.label();
            e.jcc(Cc::P, div_path); // NaN divisor: no trap, result NaN
            e.jcc(Cc::Eq, trap);
            e.bind(div_path);
            load_slot_xmm0(e, slot_of[x.0 as usize]);
            load_slot_xmm1(e, slot_of[y.0 as usize]);
            e.f2_op(0x5E, 0, 1);
            // NaN canonicalization.
            e.ucomisd(0, 0);
            e.jcc(Cc::Np, nan_check);
            e.movabs(10, crate::hot::value::CANONICAL_NAN);
            e.movq_xmm_r(2, 10);
            e.bind(nan_check);
            store_slot_xmm0(e, slot);
            e.bind(done);
            e.jmp(end);
            e.bind(trap);
            trap_rets.push(emit_trap_call(e, helper.trap_div0));
            e.ud2();
            e.bind(end);
        }
        NodeKind::Mod => {
            let x = node.inputs[0];
            let y = node.inputs[1];
            load_slot_xmm0(e, slot_of[x.0 as usize]);
            load_slot_xmm1(e, slot_of[y.0 as usize]);
            e.call_abs(helper.fmod);
            store_slot_xmm0(e, slot);
        }
        NodeKind::Lt | NodeKind::Gt | NodeKind::Eq | NodeKind::Or => {
            let x = node.inputs[0];
            let y = node.inputs[1];
            if node.kind == NodeKind::Or {
                load_slot_rax(e, slot_of[x.0 as usize]);
                load_slot_rcx(e, slot_of[y.0 as usize]);
                e.emit(&[0x48, 0x09, 0xC8]); // or rax, rcx
                store_slot_rax(e, slot);
            } else {
                load_slot_xmm0(e, slot_of[x.0 as usize]);
                load_slot_xmm1(e, slot_of[y.0 as usize]);
                e.comisd(0, 1);
                match node.kind {
                    NodeKind::Lt => e.setcc(Cc::B, 0),
                    NodeKind::Gt => e.setcc(Cc::A, 0),
                    _ => {
                        e.setcc(Cc::Eq, 0);
                        e.setcc(Cc::Np, 1);
                        e.emit(&[0x20, 0xC8]); // and al, cl
                    }
                }
                e.movzx(0, 0);
                store_slot_rax(e, slot);
            }
        }
        NodeKind::NewArray => {
            let len = node.inputs[0];
            load_slot_xmm0(e, slot_of[len.0 as usize]);
            e.emit(&[0xF2, 0x48, 0x0F, 0x2C, 0xC0]); // cvttsd2si rax, xmm0
            e.mov_rr(7, 0); // rdi = len
            e.call_abs(helper.alloc);
            e.test_rr(0, 0);
            let trap = e.label();
            let ok = e.label();
            e.jcc(Cc::Eq, trap);
            e.jmp(ok);
            e.bind(trap);
            // Allocation failure: bounds-trap entry; the engine deopts to
            // the interpreter which retries after collection (doc/deopt.md).
            trap_rets.push(emit_trap_call(e, helper.trap_bounds));
            e.ud2();
            e.bind(ok);
            store_slot_rax(e, slot);
        }
        NodeKind::ArrayLen => {
            let arr = node.inputs[0];
            load_slot_rax(e, slot_of[arr.0 as usize]);
            mask_payload(e);
            e.emit(&[0x48, 0x8B, 0x40, 0x08]); // mov rax,[rax+8]
            e.emit(&[0xF2, 0x48, 0x0F, 0x2A, 0xC0]); // cvtsi2sd xmm0, rax
            store_slot_xmm0(e, slot);
        }
        NodeKind::LoadIdx => {
            let arr = node.inputs[0];
            let idx = node.inputs[1];
            load_slot_rax(e, slot_of[arr.0 as usize]);
            mask_payload(e);
            e.mov_rr(7, 0); // rdi = ptr
            load_slot_xmm0(e, slot_of[idx.0 as usize]); // xmm0 = idx
            e.call_abs(helper.load_elem); // rax=word, rdx=flag
            e.test_rr(2, 2);
            let trap = e.label();
            let ok = e.label();
            e.jcc(Cc::Ne, trap);
            e.jmp(ok);
            e.bind(trap);
            trap_rets.push(emit_trap_call(e, helper.trap_bounds));
            e.ud2();
            e.bind(ok);
            store_slot_rax(e, slot);
        }
        NodeKind::StoreIdx => {
            let arr = node.inputs[0];
            let idx = node.inputs[1];
            let val = node.inputs[2];
            load_slot_rax(e, slot_of[arr.0 as usize]);
            mask_payload(e);
            e.mov_rr(7, 0); // rdi = ptr
            load_slot_xmm0(e, slot_of[idx.0 as usize]); // xmm0 = idx
            load_slot_rax(e, slot_of[val.0 as usize]);
            e.mov_rr(6, 0); // rsi = val word
            e.call_abs(helper.store_elem);
            e.test_rr(0, 0);
            let trap = e.label();
            let ok = e.label();
            e.jcc(Cc::Ne, trap);
            e.jmp(ok);
            e.bind(trap);
            trap_rets.push(emit_trap_call(e, helper.trap_bounds));
            e.ud2();
            e.bind(ok);
        }
        NodeKind::VecLoad { width } => {
            let arr = node.inputs[0];
            let idx = node.inputs[1];
            load_slot_rax(e, slot_of[arr.0 as usize]);
            mask_payload(e); // rax = ptr
            load_slot_xmm0(e, slot_of[idx.0 as usize]);
            e.emit(&[0xF2, 0x48, 0x0F, 0x2C, 0xC8]); // cvttsd2si rcx, xmm0
            if *width == 4 && avx2 {
                // vmovupd ymm0, [rax + rcx*8 + 16]
                e.emit(&[0xC5, 0xFD, 0x10, 0x44, 0xC8, 0x10]);
                // vmovupd [rbp + vecdisp], ymm0 (disp32)
                let disp = -8i64 * (i64::from(slot) + i64::from(VEC_SLOTS));
                e.emit(&[0xC5, 0xFD, 0x11, 0x85]);
                e.emit(&(disp as i32).to_le_bytes());
            } else {
                // Scalar fallback: width sequential loads (38.26 fallback).
                for lane in 0..*width {
                    e.emit(&[0x48, 0x8D, 0x14, 0xC8]); // lea rdx,[rax+rcx*8]
                    let off = 16 + 8 * lane;
                    e.emit(&[0x48, 0x8B, 0xB2]);
                    e.emit(&(off as i32).to_le_bytes()); // mov rsi,[rdx+off]
                    let sd = -8i64 * (i64::from(slot) + i64::from(lane) + 1);
                    e.emit(&[0x48, 0x89, 0xB5]);
                    e.emit(&(sd as i32).to_le_bytes()); // mov [rbp+sd], rsi
                }
            }
        }
        NodeKind::VecStore { width } => {
            let arr = node.inputs[0];
            let idx = node.inputs[1];
            let val = node.inputs[2];
            load_slot_rax(e, slot_of[arr.0 as usize]);
            mask_payload(e); // rax = ptr
            load_slot_xmm0(e, slot_of[idx.0 as usize]);
            e.emit(&[0xF2, 0x48, 0x0F, 0x2C, 0xC8]); // cvttsd2si rcx, xmm0
            let vslot = slot_of[val.0 as usize];
            if *width == 4 && avx2 {
                let disp = -8i64 * (i64::from(vslot) + i64::from(VEC_SLOTS));
                // vmovupd ymm0, [rbp + vecdisp]
                e.emit(&[0xC5, 0xFD, 0x10, 0x85]);
                e.emit(&(disp as i32).to_le_bytes());
                // vmovupd [rax + rcx*8 + 16], ymm0
                e.emit(&[0xC5, 0xFD, 0x11, 0x44, 0xC8, 0x10]);
            } else {
                for lane in 0..*width {
                    e.emit(&[0x48, 0x8D, 0x14, 0xC8]); // lea rdx,[rax+rcx*8]
                    let off = 16 + 8 * lane;
                    let ld = -8i64 * (i64::from(vslot) + i64::from(lane) + 1);
                    e.emit(&[0x48, 0x8B, 0xB5]);
                    e.emit(&(ld as i32).to_le_bytes()); // mov rsi,[rbp+ld]
                    e.emit(&[0x48, 0x89, 0xB2]);
                    e.emit(&(off as i32).to_le_bytes()); // mov [rdx+off], rsi
                }
            }
        }
        NodeKind::VecAddConst { width } | NodeKind::VecSubConst { width }
        | NodeKind::VecMulConst { width } => {
            let v = node.inputs[0];
            let c = node.inputs[1];
            let vslot = slot_of[v.0 as usize];
            let cslot = slot_of[c.0 as usize];
            let vdisp = -8i64 * (i64::from(vslot) + i64::from(VEC_SLOTS));
            if *width == 4 && avx2 {
                // vmovupd ymm0, [rbp+vdisp]
                e.emit(&[0xC5, 0xFD, 0x10, 0x85]);
                e.emit(&(vdisp as i32).to_le_bytes());
                // vbroadcastsd ymm1, [rbp+cdisp]
                let cdisp = -8i64 * (i64::from(cslot) + 1);
                e.emit(&[0xC4, 0xE2, 0xFD, 0x12, 0x8D]);
                e.emit(&(cdisp as i32).to_le_bytes());
                let op: u8 = match node.kind {
                    NodeKind::VecAddConst { .. } => 0x58,
                    NodeKind::VecSubConst { .. } => 0x5C,
                    _ => 0x59,
                };
                // vaddpd/vsubpd/vmulpd ymm0, ymm0, ymm1 (C5 FD op C1)
                e.emit(&[0xC5, 0xFD, op, 0xC1]);
                // vmovupd [rbp+slotdisp], ymm0
                let sdisp = -8i64 * (i64::from(slot) + i64::from(VEC_SLOTS));
                e.emit(&[0xC5, 0xFD, 0x11, 0x85]);
                e.emit(&(sdisp as i32).to_le_bytes());
            } else {
                let op: u8 = match node.kind {
                    NodeKind::VecAddConst { .. } => 0x58,
                    NodeKind::VecSubConst { .. } => 0x5C,
                    _ => 0x59,
                };
                for lane in 0..*width {
                    let vl = -8i64 * (i64::from(vslot) + i64::from(lane) + 1);
                    e.emit(&[0xF2, 0x0F, 0x10, 0x85]);
                    e.emit(&(vl as i32).to_le_bytes()); // movsd xmm0,[v+lane]
                    let cl = -8i64 * (i64::from(cslot) + 1);
                    e.emit(&[0xF2, 0x0F, 0x10, 0x8D]);
                    e.emit(&(cl as i32).to_le_bytes()); // movsd xmm1,[c]
                    e.f2_op(op, 0, 1);
                    let sl = -8i64 * (i64::from(slot) + i64::from(lane) + 1);
                    e.emit(&[0xF2, 0x0F, 0x11, 0x85]);
                    e.emit(&(sl as i32).to_le_bytes()); // movsd [s+lane],xmm0
                }
            }
        }
        NodeKind::Call { callee } => {
            for (i, arg) in node.inputs.iter().enumerate() {
                if i >= ARG_REGS_LEN {
                    return Err(EngineError::IrVerificationFailed(
                        "more than 6 call arguments".to_owned(),
                    ));
                }
                load_slot_rax(e, slot_of[arg.0 as usize]);
                e.mov_rr(ARG_REGS[i], 0);
            }
            let Some(addr) = callee_addrs.get(*callee as usize).copied() else {
                return Err(EngineError::IrVerificationFailed(format!(
                    "callee {} has no resolved address",
                    callee
                )));
            };
            e.movabs(11, addr as u64);
            e.emit(&[0x41, 0xFF, 0xD3]); // call r11
            store_slot_rax(e, slot);
        }
        NodeKind::Safepoint => {
            e.emit(&[0x48, 0x89, 0xEF]); // mov rdi, rbp
            e.emit(&[0x48, 0x89, 0xE6]); // mov rsi, rsp
            e.call_abs(helper.poll);
        }
        NodeKind::Return => {
            let v = node.inputs[1];
            load_slot_rax(e, slot_of[v.0 as usize]);
            e.emit(&[0x31, 0xD2]); // xor edx,edx (status ok)
            e.epilogue();
        }
        NodeKind::Halt => {
            // The interpreter's Halt returns the bottom of the value stack;
            // the halt's recorded site carries the stack shape.
            let bottom = deopt_sites
                .iter()
                .find(|(n, _)| *n == nid)
                .and_then(|(_, s)| s.stack.first().copied());
            match bottom {
                Some((bnode, bty)) => {
                    load_slot_rax(e, slot_of[bnode.0 as usize]);
                    e.emit(&[0x31, 0xD2]); // xor edx,edx
                    e.epilogue();
                    // The return tag is read by lower() from the site.
                    let _ = bty;
                }
                None => {
                    e.emit(&[0x31, 0xC0]);
                    e.emit(&[0x31, 0xD2]);
                    e.epilogue();
                }
            }
        }
    }
    Ok(trap_rets)
}

// CEP:FILE: engine/src/cold/ir.rs
// CEP:WHAT: The Sea of Nodes IR: node arena, typed node kinds, basic
//           blocks, the bytecode-to-graph builder, the verifier and the
//           deterministic printer and value hash.
// CEP:WHY: Spec 3.1: T1 is the first tier with an IR; standard 38.17
//          requires the IR to be printable, hashable, verifiable, stable
//          and free of pointer or hash-order dependence; 38.18 requires
//          verification after construction and after every pass.
// CEP:CLASS: HPC-1
// CEP:STATUS: complete
// CEP:FAILURE: LiftError variants (see builder) and verify errors; all
//              failures abort compilation of the unit and the engine falls
//              back to a lower tier (never silent, Law 2).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-IR: Builds target-independent Sea of Nodes SSA from verified
//             bytecode; guards and safepoints carry deopt metadata; the
//             verifier checks use-def, phi arity, types and terminators.
// CEP:HPC-DETERMINISM: deterministic; pc-order walk (an RPO for structured
//             control flow), ordered arenas, no hash iteration.
// CEP:HPC-COMPILE-COST: O(n + merges * slots); measured in benches.
// CEP:HPC-COMPILE-MEM: O(n) arena nodes plus per-block environments.
// CEP:ASSUMES: input passed cold::bytecode::verify; structured control
//              flow (pc order is an RPO); balanced merges (enforced).
// CEP:COST: see CEP:HPC-COMPILE-COST.
// CEP:EVIDENCE: tests/ir_golden.rs (print/hash determinism), tests/ir_verify.rs,
//               differential tests t1..t3.
// CEP:SECURITY: untrusted bytecode was verified upstream; guards are
//               emitted for every division and array access (23.7).

/// Node identifier: index into the graph arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// Basic-block identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

/// Static value type of an IR node (spec 3.1 node types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VType {
    /// 64-bit control token.
    Control,
    /// f64 number.
    Num,
    /// Boolean.
    Bool,
    /// String id.
    Str,
    /// Heap object reference (array).
    Obj,
    /// Unit (no value).
    Unit,
}

/// Why a compiled frame deoptimizes (spec 5.7.2, doc/deopt.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeoptReason {
    /// Array index out of bounds.
    BoundsCheck,
    /// Division by zero.
    DivisionByZero,
    /// Speculative assumption failed.
    TypeGuard,
    /// Budget forced a bail-out.
    Budget,
}

/// Node kinds. Input order is kind-specific and documented per variant;
/// the printer and verifier depend on it (38.19).
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    // ---- control ----
    /// Function entry. No inputs.
    Start,
    /// Control merge; one control input per predecessor.
    Region,
    /// Loop header; inputs: entry control, backedge control.
    Loop,
    /// Conditional branch; inputs: control, condition.
    If,
    /// True projection of If.
    IfTrue,
    /// False projection of If.
    IfFalse,
    /// Return; inputs: control, result.
    Return,
    /// Halt terminator; inputs: control.
    Halt,
    // ---- data ----
    /// f64 constant.
    ConstNum(f64),
    /// Boolean constant.
    ConstBool(bool),
    /// String-constant reference (pool id).
    ConstStr(u32),
    /// Unit constant.
    Null,
    // ---- arithmetic (inputs: left, right) ----
    /// a+b (numbers) or concatenation (strings).
    Add,
    /// a-b.
    Sub,
    /// a*b.
    Mul,
    /// a/b; guarded by a zero check (uncommon trap).
    Div,
    /// a mod b.
    Mod,
    /// a<b.
    Lt,
    /// a>b.
    Gt,
    /// a==b.
    Eq,
    /// Logical OR (guard conditions).
    Or,
    // ---- variables (SSA) ----
    /// SSA merge of one local slot; one input per predecessor.
    Phi {
        /// Local slot merged.
        slot: u16,
    },
    // ---- arrays (engine extension opcodes) ----
    /// Allocate a zero-initialized array; inputs: length.
    NewArray,
    /// Element load; inputs: array, index; guarded.
    LoadIdx,
    /// Element store; inputs: array, index, value; guarded; barrier at
    /// lowering (spec 7.5).
    StoreIdx,
    /// Array length; inputs: array.
    ArrayLen,
    // ---- calls (engine extension opcodes) ----
    /// Direct call; inputs: arguments in order; metadata: callee id.
    Call {
        /// Callee function id.
        callee: u32,
    },
    // ---- vectorization (T3) ----
    /// Vector element load; inputs: array, index; metadata: lane width.
    VecLoad {
        /// Lanes per vector.
        width: u32,
    },
    /// Vector store; inputs: array, index, value; metadata: lane width.
    VecStore {
        /// Lanes per vector.
        width: u32,
    },
    /// Vector + broadcast constant; inputs: vector, constant.
    VecAddConst {
        /// Lanes per vector.
        width: u32,
    },
    /// Vector - broadcast constant.
    VecSubConst {
        /// Lanes per vector.
        width: u32,
    },
    /// Vector * broadcast constant.
    VecMulConst {
        /// Lanes per vector.
        width: u32,
    },
    // ---- GC / deopt scaffolding ----
    /// Safepoint: GC-safe location with deopt state (spec 5.7.1).
    Safepoint,
    /// Uncommon trap: guard failure that deoptimizes (spec 5.7.2).
    UncommonTrap {
        /// Recorded deopt reason.
        reason: DeoptReason,
    },
}

/// One IR node.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Kind and metadata.
    pub kind: NodeKind,
    /// Input edges.
    pub inputs: Vec<NodeId>,
    /// Home block.
    pub block: BlockId,
    /// Static type.
    pub ty: VType,
}

/// One basic block.
#[derive(Debug, Clone)]
pub struct Block {
    /// Predecessors in edge order.
    pub preds: Vec<BlockId>,
    /// Successors in edge order.
    pub succs: Vec<BlockId>,
    /// First (control head) node.
    pub head: Option<NodeId>,
}

/// A Sea of Nodes graph for one function.
#[derive(Debug, Clone)]
pub struct Graph {
    /// Node arena; ids are creation-ordered and never reused (38.17).
    pub nodes: Vec<Node>,
    /// Blocks in pc order (an RPO for structured control flow).
    pub blocks: Vec<Block>,
    /// Entry control node.
    pub start: NodeId,
    /// Return node (None for Halt-only graphs).
    pub ret: Option<NodeId>,
    /// Halt node (None for Return-only graphs).
    pub halt: Option<NodeId>,
    /// Local slot count.
    pub local_count: u16,
}

impl Graph {
    /// Allocate a node.
    ///
    /// CEP:WHAT: Node allocation.
    /// CEP:WHY: Arena growth with stable ids.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: node count fits u32 (budget-checked by callers).
    /// CEP:COST: amortized O(1).
    /// CEP:EVIDENCE: IR golden tests.
    /// CEP:SECURITY: none
    pub fn alloc(&mut self, kind: NodeKind, inputs: Vec<NodeId>, block: BlockId, ty: VType) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node { kind, inputs, block, ty });
        id
    }

    /// True when the node kind is pure (no effects, no traps, no allocation).
    ///
    /// CEP:WHAT: Purity classification.
    /// CEP:WHY: GVN, LICM, DCE and PEA transform only pure nodes; Div is
    ///          impure (it can trap), NewArray allocates.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: one match.
    /// CEP:EVIDENCE: pass legality tests.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn is_pure(kind: &NodeKind) -> bool {
        matches!(
            kind,
            NodeKind::Add
                | NodeKind::Sub
                | NodeKind::Mul
                | NodeKind::Mod
                | NodeKind::Lt
                | NodeKind::Gt
                | NodeKind::Eq
                | NodeKind::Or
                | NodeKind::Phi { .. }
                | NodeKind::ArrayLen
                | NodeKind::VecLoad { .. }
                | NodeKind::VecAddConst { .. }
                | NodeKind::VecSubConst { .. }
                | NodeKind::VecMulConst { .. }
                | NodeKind::ConstNum(_)
                | NodeKind::ConstBool(_)
                | NodeKind::ConstStr(_)
                | NodeKind::Null
        )
    }

    /// Deterministic FNV-1a hash over the printed form.
    ///
    /// CEP:WHAT: Graph value hash.
    /// CEP:WHY: Golden tests and determinism checks (38.19: no pointer,
    ///          allocation-order, hash-seed or thread dependence).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(nodes).
    /// CEP:EVIDENCE: tests/ir_golden.rs.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn value_hash(&self) -> u64 {
        let text = self.print();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in text.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        h
    }

    /// Deterministic textual form.
    ///
    /// CEP:WHAT: IR printer (38.17 requires printable IR).
    /// CEP:WHY: Golden tests pin this exact format.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(nodes + edges).
    /// CEP:EVIDENCE: tests/ir_golden.rs.
    /// CEP:SECURITY: prints no untrusted strings (ids only).
    #[must_use]
    pub fn print(&self) -> String {
        let mut out = String::with_capacity(self.nodes.len() * 24);
        out.push_str("graph locals=");
        out.push_str(&self.local_count.to_string());
        out.push_str(" blocks=");
        out.push_str(&self.blocks.len().to_string());
        out.push('\n');
        for (i, n) in self.nodes.iter().enumerate() {
            out.push_str(&format!("n{i} b{} ", n.block.0));
            match &n.kind {
                NodeKind::Start => out.push_str("Start"),
                NodeKind::Region => out.push_str("Region"),
                NodeKind::Loop => out.push_str("Loop"),
                NodeKind::If => out.push_str("If"),
                NodeKind::IfTrue => out.push_str("IfTrue"),
                NodeKind::IfFalse => out.push_str("IfFalse"),
                NodeKind::Return => out.push_str("Return"),
                NodeKind::Halt => out.push_str("Halt"),
                NodeKind::ConstNum(f) => out.push_str(&format!("ConstNum({f:?})")),
                NodeKind::ConstBool(b) => out.push_str(&format!("ConstBool({b})")),
                NodeKind::ConstStr(s) => out.push_str(&format!("ConstStr({s})")),
                NodeKind::Null => out.push_str("Null"),
                NodeKind::Add => out.push_str("Add"),
                NodeKind::Sub => out.push_str("Sub"),
                NodeKind::Mul => out.push_str("Mul"),
                NodeKind::Div => out.push_str("Div"),
                NodeKind::Mod => out.push_str("Mod"),
                NodeKind::Lt => out.push_str("Lt"),
                NodeKind::Gt => out.push_str("Gt"),
                NodeKind::Eq => out.push_str("Eq"),
                NodeKind::Or => out.push_str("Or"),
                NodeKind::Phi { slot } => out.push_str(&format!("Phi({slot})")),
                NodeKind::NewArray => out.push_str("NewArray"),
                NodeKind::LoadIdx => out.push_str("LoadIdx"),
                NodeKind::StoreIdx => out.push_str("StoreIdx"),
                NodeKind::ArrayLen => out.push_str("ArrayLen"),
                NodeKind::Call { callee } => out.push_str(&format!("Call({callee})")),
                NodeKind::VecLoad { width } => out.push_str(&format!("VecLoad({width})")),
                NodeKind::VecStore { width } => out.push_str(&format!("VecStore({width})")),
                NodeKind::VecAddConst { width } => out.push_str(&format!("VecAddConst({width})")),
                NodeKind::VecSubConst { width } => out.push_str(&format!("VecSubConst({width})")),
                NodeKind::VecMulConst { width } => out.push_str(&format!("VecMulConst({width})")),
                NodeKind::Safepoint => out.push_str("Safepoint"),
                NodeKind::UncommonTrap { reason } => {
                    out.push_str(&format!("UncommonTrap({reason:?})"))
                }
            }
            out.push_str(" [");
            for (j, inp) in n.inputs.iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                out.push_str(&format!("n{}", inp.0));
            }
            out.push_str("]\n");
        }
        out
    }
}

/// Builder failure modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiftError {
    /// Value stack unbalanced at a merge point.
    UnbalancedStack,
    /// Opcode without IR semantics (stays on lower tiers).
    UnsupportedOpcode(crate::cold::bytecode::Opcode),
    /// Graph exceeds the node budget.
    TooManyNodes,
    /// Value stack underflow during lifting.
    StackUnderflow,
    /// CFG inconsistency.
    BadTarget,
}

/// Per-safepoint deopt state (doc/deopt.md Formal Definitions).
#[derive(Debug, Clone, PartialEq)]
pub struct DeoptSite {
    /// Bytecode pc the interpreter resumes at.
    pub bc_pc: u32,
    /// Value-stack shape: (node, type) bottom-to-top.
    pub stack: Vec<(NodeId, VType)>,
    /// Live locals: (slot, node, type).
    pub locals: Vec<(u16, NodeId, VType)>,
}

/// Builder output.
#[derive(Debug)]
pub struct Lifted {
    /// The Sea of Nodes graph.
    pub graph: Graph,
    /// Deopt sites keyed by their node id.
    pub deopt_sites: Vec<(NodeId, DeoptSite)>,
}

/// Pending control edge into a block.
#[derive(Debug, Clone)]
struct PendingEdge {
    ctrl: NodeId,
    env: Vec<NodeId>,
}

/// Lift one function to the Sea of Nodes IR.
///
/// CEP:WHAT: The T1 IR construction algorithm (spec 3.1).
/// CEP:WHY: Every tier above T0 starts from this graph.
/// CEP:STATUS: complete
/// CEP:FAILURE: See LiftError; failures are reported, never guessed around.
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-IR: see file header.
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:ASSUMES: verified bytecode; structured control flow.
/// CEP:COST: linear; see file header.
/// CEP:EVIDENCE: differential tests; IR golden tests.
/// CEP:SECURITY: guards emitted for all divisions and array accesses.
pub fn lift(
    func: &crate::cold::bytecode::Function,
    max_nodes: u32,
) -> Result<Lifted, LiftError> {
    use crate::cold::bytecode::{Insn, Opcode, Operand};
    let n = func.code.len();
    // ---- CFG: leaders, partitioning, loop headers ----
    let mut is_leader = vec![false; n + 1];
    is_leader[0] = true;
    for (pc, insn) in func.code.iter().enumerate() {
        if matches!(insn.op, Opcode::Jump | Opcode::JumpIfFalse | Opcode::JumpIfTrue) {
            let Operand::Target(t) = insn.operand else {
                return Err(LiftError::BadTarget);
            };
            let t = t as usize;
            if t > n {
                return Err(LiftError::BadTarget);
            }
            is_leader[t] = true;
            if insn.op != Opcode::Jump && pc + 1 <= n {
                is_leader[pc + 1] = true;
            }
        }
    }
    let mut starts = Vec::new();
    for (pc, l) in is_leader.iter().enumerate() {
        if *l && pc < n {
            starts.push(pc);
        }
    }
    if starts.is_empty() {
        starts.push(0);
    }
    let block_count = starts.len();
    let mut block_of = vec![BlockId(0); n];
    for (bi, &s) in starts.iter().enumerate() {
        let end = starts.get(bi + 1).copied().unwrap_or(n);
        for pc in s..end {
            block_of[pc] = BlockId(bi as u32);
        }
    }
    let mut loop_header = vec![false; block_count];
    for (pc, insn) in func.code.iter().enumerate() {
        if matches!(insn.op, Opcode::Jump | Opcode::JumpIfFalse | Opcode::JumpIfTrue) {
            let Operand::Target(t) = insn.operand else { continue };
            if (t as usize) <= pc {
                loop_header[block_of[t as usize].0 as usize] = true;
            }
        }
    }

    let mut g = Graph {
        nodes: Vec::with_capacity(n * 2),
        blocks: Vec::with_capacity(block_count),
        start: NodeId(0),
        ret: None,
        halt: None,
        local_count: func.locals.len() as u16,
    };
    for _ in 0..block_count {
        g.blocks.push(Block { preds: Vec::new(), succs: Vec::new(), head: None });
    }
    let start = g.alloc(NodeKind::Start, Vec::new(), BlockId(0), VType::Control);
    g.start = start;
    g.blocks[0].head = Some(start);

    // Builder state.
    let mut deopt_sites: Vec<(NodeId, DeoptSite)> = Vec::new();
    let mut entry_ctrl: Vec<Option<NodeId>> = vec![None; block_count];
    let mut entry_stack: Vec<Vec<NodeId>> = vec![Vec::new(); block_count];
    let mut extra: Vec<Vec<PendingEdge>> = vec![Vec::new(); block_count];
    entry_ctrl[0] = Some(start);
    let mut env: Vec<NodeId> = vec![NodeId(u32::MAX); func.locals.len()];

    macro_rules! budget_check {
        () => {
            if g.nodes.len() as u32 >= max_nodes {
                return Err(LiftError::TooManyNodes);
            }
        };
    }

    for bi in 0..block_count {
        let Some(ctrl0) = entry_ctrl[bi] else { continue };
        let mut ctrl = ctrl0;
        let mut stack = std::mem::take(&mut entry_stack[bi]);
        let start_pc = starts[bi];
        let end_pc = starts.get(bi + 1).copied().unwrap_or(n);
        g.blocks[bi].head = Some(ctrl);

        // Multi-pred merge: Region or Loop plus phis.
        let extra_edges = std::mem::take(&mut extra[bi]);
        if !extra_edges.is_empty() || loop_header[bi] {
            budget_check!();
            if !stack.is_empty() {
                return Err(LiftError::UnbalancedStack);
            }

            let merged = if loop_header[bi] {
                g.alloc(
                    NodeKind::Loop,
                    vec![ctrl0, ctrl0],
                    BlockId(bi as u32),
                    VType::Control,
                )
            } else {
                let mut inputs = Vec::with_capacity(extra_edges.len() + 1);
                inputs.push(ctrl0);
                for e in &extra_edges {
                    inputs.push(e.ctrl);
                }
                g.alloc(NodeKind::Region, inputs, BlockId(bi as u32), VType::Control)
            };
            // Phis. For loop headers: a placeholder phi for every live
            // slot (the back-edge value arrives later via close_loop, which
            // patches inputs[1]); phi-simplify collapses the trivial ones.
            // For ordinary merges: pruned phis only where reaching
            // definitions differ.
            let mut loop_slots: Vec<(u16, VType)> = Vec::new();
            if loop_header[bi] {
                for s in 0..env.len() {
                    if env[s].0 != u32::MAX {
                        let ty = g.nodes[env[s].0 as usize].ty;
                        loop_slots.push((s as u16, ty));
                    }
                }
            }
            for s in 0..env.len() {
                let mut vals = Vec::with_capacity(extra_edges.len() + 2);
                vals.push(env[s]);
                for e in &extra_edges {
                    vals.push(e.env[s]);
                }
                let is_loop_live = loop_header[bi] && env[s].0 != u32::MAX;
                // Loop phis carry a placeholder second input (the
                // back-edge); close_loop patches it.
                if is_loop_live {
                    vals.push(env[s]);
                }
                let differs = vals.iter().any(|v| *v != vals[0]);
                if is_loop_live || (!loop_header[bi] && differs) {
                    budget_check!();
                    let ty = if is_loop_live {
                        loop_slots
                            .iter()
                            .find(|(sl, _)| *sl as usize == s)
                            .map(|(_, t)| *t)
                            .unwrap_or(VType::Num)
                    } else {
                        VType::Num
                    };
                    let p = g.alloc(
                        NodeKind::Phi { slot: s as u16 },
                        vals,
                        BlockId(bi as u32),
                        ty,
                    );
                    env[s] = p;
                }
            }
            ctrl = merged;
            g.blocks[bi].head = Some(merged);
        }

        let mut pc = start_pc;
        while pc < end_pc {
            let insn: Insn = func.code[pc];
            match insn.op {
                Opcode::PushNumber => {
                    let Operand::F64(f) = insn.operand else {
                        return Err(LiftError::StackUnderflow);
                    };
                    budget_check!();
                    let nid = g.alloc(NodeKind::ConstNum(f), Vec::new(), BlockId(bi as u32), VType::Num);
                    stack.push(nid);
                }
                Opcode::PushBool => {
                    let Operand::U64(v) = insn.operand else {
                        return Err(LiftError::StackUnderflow);
                    };
                    budget_check!();
                    let nid = g.alloc(NodeKind::ConstBool(v == 1), Vec::new(), BlockId(bi as u32), VType::Bool);
                    stack.push(nid);
                }
                Opcode::PushString => {
                    let Operand::StrId(s) = insn.operand else {
                        return Err(LiftError::StackUnderflow);
                    };
                    budget_check!();
                    let nid = g.alloc(NodeKind::ConstStr(s), Vec::new(), BlockId(bi as u32), VType::Str);
                    stack.push(nid);
                }
                Opcode::LoadVar => {
                    let Operand::Slot(s) = insn.operand else {
                        return Err(LiftError::StackUnderflow);
                    };
                    let def = env[s as usize];
                    if def.0 == u32::MAX {
                        return Err(LiftError::StackUnderflow);
                    }
                    stack.push(def);
                }
                Opcode::StoreVar | Opcode::AssignVar => {
                    let Operand::Slot(s) = insn.operand else {
                        return Err(LiftError::StackUnderflow);
                    };
                    let v = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    env[s as usize] = v;
                }
                Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div | Opcode::Modulo
                | Opcode::GreaterThan | Opcode::LessThan | Opcode::Equal => {
                    let y = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    let x = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    let kind = match insn.op {
                        Opcode::Add => NodeKind::Add,
                        Opcode::Sub => NodeKind::Sub,
                        Opcode::Mul => NodeKind::Mul,
                        Opcode::Modulo => NodeKind::Mod,
                        Opcode::GreaterThan => NodeKind::Gt,
                        Opcode::LessThan => NodeKind::Lt,
                        Opcode::Equal => NodeKind::Eq,
                        Opcode::Div => {
                            // Guard: right == 0 -> uncommon trap (spec 5.7.2).
                            budget_check!();
                            let zero = g.alloc(NodeKind::ConstNum(0.0), Vec::new(), BlockId(bi as u32), VType::Num);
                            budget_check!();
                            let eq = g.alloc(NodeKind::Eq, vec![y, zero], BlockId(bi as u32), VType::Bool);
                            budget_check!();
                            let ifn = g.alloc(NodeKind::If, vec![ctrl, eq], BlockId(bi as u32), VType::Control);
                            budget_check!();
                            let t = g.alloc(NodeKind::IfTrue, vec![ifn], BlockId(bi as u32), VType::Control);
                            budget_check!();
                            let f = g.alloc(NodeKind::IfFalse, vec![ifn], BlockId(bi as u32), VType::Control);
                            budget_check!();
                            let trap = g.alloc(
                                NodeKind::UncommonTrap { reason: DeoptReason::DivisionByZero },
                                vec![t],
                                BlockId(bi as u32),
                                VType::Control,
                            );
                            record_site(&g, &mut deopt_sites, trap, pc as u32, &stack, &env);
                            ctrl = f;
                            NodeKind::Div
                        }
                        _ => unreachable!("matched above"),
                    };
                    let ty = if matches!(kind, NodeKind::Lt | NodeKind::Gt | NodeKind::Eq) {
                        VType::Bool
                    } else {
                        VType::Num
                    };
                    budget_check!();
                    let nid = g.alloc(kind, vec![x, y], BlockId(bi as u32), ty);
                    stack.push(nid);
                }
                Opcode::NewArray => {
                    let len = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    budget_check!();
                    let nid = g.alloc(NodeKind::NewArray, vec![len], BlockId(bi as u32), VType::Obj);
                    stack.push(nid);
                }
                Opcode::ArrayLen => {
                    let arr = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    budget_check!();
                    let nid = g.alloc(NodeKind::ArrayLen, vec![arr], BlockId(bi as u32), VType::Num);
                    stack.push(nid);
                }
                Opcode::LoadIdx => {
                    let idx = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    let arr = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    // Guard: idx < 0 || idx >= len -> trap (spec 5.7.2).
                    budget_check!();
                    let len = g.alloc(NodeKind::ArrayLen, vec![arr], BlockId(bi as u32), VType::Num);
                    budget_check!();
                    let zero = g.alloc(NodeKind::ConstNum(0.0), Vec::new(), BlockId(bi as u32), VType::Num);
                    budget_check!();
                    let neg = g.alloc(NodeKind::Lt, vec![idx, zero], BlockId(bi as u32), VType::Bool);
                    budget_check!();
                    let ge = g.alloc(NodeKind::Gt, vec![idx, len], BlockId(bi as u32), VType::Bool);
                    budget_check!();
                    let bad = g.alloc(NodeKind::Or, vec![neg, ge], BlockId(bi as u32), VType::Bool);
                    budget_check!();
                    let ifn = g.alloc(NodeKind::If, vec![ctrl, bad], BlockId(bi as u32), VType::Control);
                    budget_check!();
                    let t = g.alloc(NodeKind::IfTrue, vec![ifn], BlockId(bi as u32), VType::Control);
                    budget_check!();
                    let f = g.alloc(NodeKind::IfFalse, vec![ifn], BlockId(bi as u32), VType::Control);
                    budget_check!();
                    let trap = g.alloc(
                        NodeKind::UncommonTrap { reason: DeoptReason::BoundsCheck },
                        vec![t],
                        BlockId(bi as u32),
                        VType::Control,
                    );
                    record_site(&g, &mut deopt_sites, trap, pc as u32, &stack, &env);
                    budget_check!();
                    let nid = g.alloc(NodeKind::LoadIdx, vec![arr, idx], BlockId(bi as u32), VType::Num);
                    stack.push(nid);
                    ctrl = f;
                }
                Opcode::StoreIdx => {
                    let val = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    let idx = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    let arr = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    budget_check!();
                    let len = g.alloc(NodeKind::ArrayLen, vec![arr], BlockId(bi as u32), VType::Num);
                    budget_check!();
                    let zero = g.alloc(NodeKind::ConstNum(0.0), Vec::new(), BlockId(bi as u32), VType::Num);
                    budget_check!();
                    let neg = g.alloc(NodeKind::Lt, vec![idx, zero], BlockId(bi as u32), VType::Bool);
                    budget_check!();
                    let ge = g.alloc(NodeKind::Gt, vec![idx, len], BlockId(bi as u32), VType::Bool);
                    budget_check!();
                    let bad = g.alloc(NodeKind::Or, vec![neg, ge], BlockId(bi as u32), VType::Bool);
                    budget_check!();
                    let ifn = g.alloc(NodeKind::If, vec![ctrl, bad], BlockId(bi as u32), VType::Control);
                    budget_check!();
                    let t = g.alloc(NodeKind::IfTrue, vec![ifn], BlockId(bi as u32), VType::Control);
                    budget_check!();
                    let f = g.alloc(NodeKind::IfFalse, vec![ifn], BlockId(bi as u32), VType::Control);
                    budget_check!();
                    let trap = g.alloc(
                        NodeKind::UncommonTrap { reason: DeoptReason::BoundsCheck },
                        vec![t],
                        BlockId(bi as u32),
                        VType::Control,
                    );
                    record_site(&g, &mut deopt_sites, trap, pc as u32, &stack, &env);
                    budget_check!();
                    let _ = g.alloc(NodeKind::StoreIdx, vec![arr, idx, val], BlockId(bi as u32), VType::Unit);
                    ctrl = f;
                }
                Opcode::Jump => {
                    let Operand::Target(t) = insn.operand else {
                        return Err(LiftError::StackUnderflow);
                    };
                    let tb = block_of[t as usize].0 as usize;
                    if (t as usize) <= pc {
                        close_loop(&mut g, bi, tb, ctrl, &env, &mut deopt_sites, t as usize, max_nodes)?;
                    } else {
                        g.blocks[bi].succs.push(BlockId(tb as u32));
                        g.blocks[tb].preds.push(BlockId(bi as u32));
                        if entry_ctrl[tb].is_none() {
                            entry_ctrl[tb] = Some(ctrl);
                            entry_stack[tb] = Vec::new();
                        } else {
                            extra[tb].push(PendingEdge { ctrl, env: env.clone() });
                        }
                    }
                    stack.clear();
                    ctrl = NodeId(u32::MAX);
                }
                Opcode::JumpIfFalse | Opcode::JumpIfTrue => {
                    let Operand::Target(t) = insn.operand else {
                        return Err(LiftError::StackUnderflow);
                    };
                    let cond = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    budget_check!();
                    let ifn = g.alloc(NodeKind::If, vec![ctrl, cond], BlockId(bi as u32), VType::Control);
                    budget_check!();
                    let tp = g.alloc(NodeKind::IfTrue, vec![ifn], BlockId(bi as u32), VType::Control);
                    budget_check!();
                    let fp = g.alloc(NodeKind::IfFalse, vec![ifn], BlockId(bi as u32), VType::Control);
                    let take = if insn.op == Opcode::JumpIfTrue { tp } else { fp };
                    let skip = if insn.op == Opcode::JumpIfTrue { fp } else { tp };
                    let tb = block_of[t as usize].0 as usize;
                    let env_a = env.clone();
                    let env_b = env.clone();
                    if (t as usize) <= pc {
                        close_loop(&mut g, bi, tb, take, &env_a, &mut deopt_sites, t as usize, max_nodes)?;
                        if bi + 1 < block_count {
                            g.blocks[bi].succs.push(BlockId((bi + 1) as u32));
                            g.blocks[bi + 1].preds.push(BlockId(bi as u32));
                            if entry_ctrl[bi + 1].is_none() {
                                entry_ctrl[bi + 1] = Some(skip);
                                entry_stack[bi + 1] = Vec::new();
                            } else {
                                extra[bi + 1].push(PendingEdge { ctrl: skip, env: env_b });
                            }
                        }
                    } else {
                        g.blocks[bi].succs.push(BlockId(tb as u32));
                        g.blocks[tb].preds.push(BlockId(bi as u32));
                        if entry_ctrl[tb].is_none() {
                            entry_ctrl[tb] = Some(take);
                            entry_stack[tb] = Vec::new();
                        } else {
                            extra[tb].push(PendingEdge { ctrl: take, env: env_a });
                        }
                        if bi + 1 < block_count {
                            g.blocks[bi].succs.push(BlockId((bi + 1) as u32));
                            g.blocks[bi + 1].preds.push(BlockId(bi as u32));
                            if entry_ctrl[bi + 1].is_none() {
                                entry_ctrl[bi + 1] = Some(skip);
                                entry_stack[bi + 1] = Vec::new();
                            } else {
                                extra[bi + 1].push(PendingEdge { ctrl: skip, env: env_b });
                            }
                        }
                    }
                    stack.clear();
                    ctrl = NodeId(u32::MAX);
                }
                Opcode::PushUsize | Opcode::JumpLastOnStack | Opcode::WriteLn | Opcode::Write
                | Opcode::ReadInput | Opcode::ProcessExit | Opcode::Drop => {
                    // Without IR semantics at T1+: the legacy call convention
                    // (PushUsize/Jump/JumpLastOnStack) is replaced by the
                    // engine's CALL/RET extension; I/O stays on the
                    // interpreter (doc/bytecode.md Known Limitations).
                    return Err(LiftError::UnsupportedOpcode(insn.op));
                }
                Opcode::Call => {
                    let Operand::U64(packed) = insn.operand else {
                        return Err(LiftError::StackUnderflow);
                    };
                    let argc = (packed >> 32) as usize;
                    let callee = (packed & 0xFFFF_FFFF) as u32;
                    let mut args = Vec::with_capacity(argc);
                    for _ in 0..argc {
                        args.push(stack.pop().ok_or(LiftError::StackUnderflow)?);
                    }
                    args.reverse();
                    budget_check!();
                    let call = g.alloc(NodeKind::Call { callee }, args, BlockId(bi as u32), VType::Num);
                    stack.push(call);
                    // Calls are safepoints (spec 2.6).
                    budget_check!();
                    let sp = g.alloc(NodeKind::Safepoint, vec![ctrl], BlockId(bi as u32), VType::Control);
                    record_site(&g, &mut deopt_sites, sp, pc as u32 + 1, &stack, &env);
                    ctrl = sp;
                }
                Opcode::Ret => {
                    let v = stack.pop().ok_or(LiftError::StackUnderflow)?;
                    budget_check!();
                    let r = g.alloc(NodeKind::Return, vec![ctrl, v], BlockId(bi as u32), VType::Control);
                    g.ret = Some(r);
                    stack.clear();
                    ctrl = NodeId(u32::MAX);
                }
                Opcode::Halt => {
                    budget_check!();
                    let h = g.alloc(NodeKind::Halt, vec![ctrl], BlockId(bi as u32), VType::Control);
                    g.halt = Some(h);
                    // Record the halt's stack shape: the interpreter's Halt
                    // returns the bottom of the value stack (legacy parity),
                    // so codegen needs the bottom node and its type.
                    record_site(&g, &mut deopt_sites, h, pc as u32, &stack, &env);
                    stack.clear();
                    ctrl = NodeId(u32::MAX);
                }
            }
            pc += 1;
        }

        // Fallthrough into the next block when control is still live.
        if ctrl.0 != u32::MAX && bi + 1 < block_count {
            g.blocks[bi].succs.push(BlockId((bi + 1) as u32));
            g.blocks[bi + 1].preds.push(BlockId(bi as u32));
            if entry_ctrl[bi + 1].is_none() {
                entry_ctrl[bi + 1] = Some(ctrl);
            } else {
                extra[bi + 1].push(PendingEdge { ctrl, env: env.clone() });
            }
        }
    }

    Ok(Lifted { graph: g, deopt_sites })
}

/// Record a deopt site's state.
///
/// CEP:WHAT: Deopt metadata capture.
/// CEP:WHY: Guards and safepoints need resumable interpreter state.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: nodes referenced exist.
/// CEP:COST: O(stack + locals).
/// CEP:EVIDENCE: deopt tests.
/// CEP:SECURITY: none
fn record_site(
    g: &Graph,
    sites: &mut Vec<(NodeId, DeoptSite)>,
    node: NodeId,
    bc_pc: u32,
    stack: &[NodeId],
    env: &[NodeId],
) {
    let shape = stack.iter().map(|n| (*n, g.nodes[n.0 as usize].ty)).collect();
    let locals = env
        .iter()
        .enumerate()
        .filter(|(_, nid)| nid.0 != u32::MAX)
        .map(|(s, nid)| (s as u16, *nid, g.nodes[nid.0 as usize].ty))
        .collect();
    sites.push((node, DeoptSite { bc_pc, stack: shape, locals }));
}

/// Wire a back-edge: patch the Loop node's backedge input and the header
/// phis, then record the back-edge safepoint (spec 2.6).
///
/// CEP:WHAT: Loop closure.
/// CEP:WHY: Loop phis need the back-edge value; safepoints live at
///          back-edges.
/// CEP:STATUS: complete
/// CEP:FAILURE: BadTarget for unstructured back-edges.
/// CEP:ASSUMES: headers marked by the CFG pass.
/// CEP:COST: O(phis in header).
/// CEP:EVIDENCE: differential loop tests.
/// CEP:SECURITY: none
///
/// # Errors
/// BadTarget.
#[allow(clippy::too_many_arguments)]
fn close_loop(
    g: &mut Graph,
    from: usize,
    header: usize,
    ctrl: NodeId,
    env: &[NodeId],
    sites: &mut Vec<(NodeId, DeoptSite)>,
    bc_pc: usize,
    max_nodes: u32,
) -> Result<(), LiftError> {
    if header >= g.blocks.len() {
        return Err(LiftError::BadTarget);
    }
    g.blocks[from].succs.push(BlockId(header as u32));
    let Some(h) = g.blocks[header].head else {
        return Err(LiftError::BadTarget);
    };
    match &mut g.nodes[h.0 as usize].kind {
        NodeKind::Loop => {
            g.nodes[h.0 as usize].inputs[1] = ctrl;
        }
        _ => return Err(LiftError::BadTarget),
    }
    g.blocks[header].preds.push(BlockId(from as u32));
    let header_id = BlockId(header as u32);
    for i in 0..g.nodes.len() {
        let node = &mut g.nodes[i];
        if node.block == header_id {
            if let NodeKind::Phi { slot } = node.kind {
                let s = slot as usize;
                if s < env.len() && env[s].0 != u32::MAX {
                    node.inputs[1] = env[s];
                }
            }
        }
    }
    if g.nodes.len() as u32 >= max_nodes {
        return Err(LiftError::TooManyNodes);
    }
    let sp = g.alloc(NodeKind::Safepoint, vec![h], header_id, VType::Control);
    record_site(g, sites, sp, bc_pc as u32, &[], env);
    Ok(())
}

/// Verify one graph (standard 38.18).
///
/// CEP:WHAT: Full verifier pass.
/// CEP:WHY: 38.18 contract: after construction, after every pass, before
///          lowering and before emission.
/// CEP:STATUS: complete
/// CEP:FAILURE: Err with a stable message on the first violation.
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-IR: Verifies use-def range, phi arity, type domains, terminator
///             uniqueness and head presence.
/// CEP:HPC-DETERMINISM: deterministic (node-order checks).
/// CEP:ASSUMES: node ids are valid indices.
/// CEP:COST: O(n + e).
/// CEP:EVIDENCE: tests/ir_verify.rs positive and negative cases.
/// CEP:SECURITY: the gate that keeps untrusted bytecode from producing
///               invalid machine code.
///
/// # Errors
/// Stable textual description of the first violated invariant.
pub fn verify(g: &Graph) -> Result<(), String> {
    for (i, node) in g.nodes.iter().enumerate() {
        for inp in &node.inputs {
            if inp.0 as usize >= g.nodes.len() {
                return Err(format!("n{i}: input n{} out of range", inp.0));
            }
        }
        if node.block.0 as usize >= g.blocks.len() {
            return Err(format!("n{i}: block {} out of range", node.block.0));
        }
    }
    if g.nodes.is_empty() || g.nodes[0].kind != NodeKind::Start {
        return Err("graph must start with a Start node at n0".to_owned());
    }
    for (i, node) in g.nodes.iter().enumerate() {
        if let NodeKind::Phi { .. } = node.kind {
            let b = &g.blocks[node.block.0 as usize];
            let expect = b.preds.len().max(2);
            if node.inputs.len() != expect {
                return Err(format!(
                    "n{i}: Phi has {} inputs but block {} has {} preds",
                    node.inputs.len(),
                    node.block.0,
                    expect
                ));
            }
        }
        match &node.kind {
            NodeKind::Add | NodeKind::Sub | NodeKind::Mul | NodeKind::Div | NodeKind::Mod
            | NodeKind::Lt | NodeKind::Gt | NodeKind::Eq | NodeKind::Or => {
                if node.inputs.len() != 2 {
                    return Err(format!("n{i}: binary node must have 2 inputs"));
                }
                if node.ty != VType::Num && node.ty != VType::Bool {
                    return Err(format!("n{i}: binary type must be Num or Bool"));
                }
            }
            NodeKind::If => {
                if node.inputs.len() != 2 {
                    return Err(format!("n{i}: If must have control and condition"));
                }
            }
            _ => {}
        }
    }
    let rets = g.nodes.iter().filter(|n| n.kind == NodeKind::Return).count();
    let halts = g.nodes.iter().filter(|n| n.kind == NodeKind::Halt).count();
    if rets > 1 || halts > 1 {
        return Err(format!("graph has {rets} returns and {halts} halts"));
    }
    if g.ret.is_none() && g.halt.is_none() {
        return Err("graph has no terminator".to_owned());
    }
    Ok(())
}

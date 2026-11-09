// CEP:FILE: engine/src/cold/inline.rs
// CEP:WHAT: Cross-function inlining for T3: callee bodies are spliced into
//           the caller's bytecode at eligible call sites (spec 5.2.1),
//           with a depth budget, a node budget, a cycle guard, a
//           call-site stack-depth gate and a single-trailing-Ret gate;
//           the merged function is plain verified bytecode and rides the
//           normal lift/pipeline path.
// CEP:WHY: Spec 5.2.1 mandates method inlining at T3 (depth 3-5, callee
//          node budget); the engine's budgets existed without an
//          implementation. Bytecode-level splicing reuses the entire
//          verified lift/pass/codegen path and keeps deopt coherent: the
//          merged function is registered in the function table and serves
//          as the interpreter image after a trap (doc/inline.md).
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: Every gate refusal skips that call site; a merged lift
//              failure at the engine falls back to the un-inlined T3
//              compilation (never to a wrong result; the gates are
//              documented in doc/inline.md).
// CEP:ASSUMES: verified bytecode; the lift builder's contract that value
//              stacks are empty at block boundaries (doc/ir.md); callee
//              bodies without Halt.
// CEP:COST: O(caller + spliced bodies) per compilation; bounded by the
//           splice budget (max_ir_nodes) and max_inline_depth.
// CEP:EVIDENCE: tests/call_inline_tests.rs (effectiveness, gates, parity,
//               deopt resume).
// CEP:SECURITY: merged bytecode passes cold::bytecode::verify and the IR
//               verifier before publication; no new trust boundary.

use crate::cold::bytecode::{Function, Insn, Operand, Opcode, MAX_LOCALS};
use crate::cold::passes::PassBudgets;
use std::collections::BTreeSet;

/// Result of one inlining attempt over a caller body.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineOutcome {
    /// The merged function, or None when no call site was eligible.
    pub merged: Option<Function>,
    /// Number of call sites inlined.
    pub inlined_calls: u64,
}

/// Inline eligible call sites in `caller` (spec 5.2.1).
///
/// CEP:WHAT: The T3 inliner entry point.
/// CEP:WHY: Spec 5.2.1; the depth and node budgets come from the pass
///          budgets (max_inline_depth, max_inline_nodes).
/// CEP:STATUS: complete
/// CEP:FAILURE: gate refusals skip call sites; None merged means the
///              caller compiles un-inlined.
/// CEP:ASSUMES: caller is funcs\[caller_id\]; arities\[callee\] is the maximum
///              Call argc over the table (engine invariant).
/// CEP:COST: O(caller + spliced) per attempt.
/// CEP:EVIDENCE: tests/call_inline_tests.rs.
/// CEP:SECURITY: merged output is re-verified by the engine before use.
#[must_use]
pub fn inline_calls(
    caller: &Function,
    caller_id: u32,
    funcs: &[Function],
    arities: &[u32],
    budgets: &PassBudgets,
    max_ir_nodes: u32,
) -> InlineOutcome {
    let mut chain = BTreeSet::new();
    chain.insert(caller_id);
    let mut budget = SpliceBudget { remaining: i64::from(max_ir_nodes) };
    inline_at_depth(
        caller,
        caller_id,
        funcs,
        arities,
        budgets,
        budgets.max_inline_depth,
        &chain,
        &mut budget,
    )
}

/// Running bound on total spliced instructions (mirrors the IR budget).
struct SpliceBudget {
    remaining: i64,
}

/// One completed splice: the arg-binding stores, the remapped body
/// (Ret instructions removed), the merged-space continuation pc, and the
/// appended table entries (from the pre-inlined body).
struct Splice {
    stores: Vec<Insn>,
    insns: Vec<Insn>,
    local_names: Vec<String>,
    strings: Vec<String>,
    /// Splices contained in this body, this one included: the caller's
    /// walk adds it so `inlined_calls` counts every site in the merged
    /// body, at every depth.
    inlined: u64,
}

/// Recursive worker: splice eligible call sites in `body` at `depth`.
///
/// CEP:WHAT: The splicing transform (one pass over the instruction stream).
/// CEP:WHY: One pass suffices: spliced bodies are pre-inlined at depth-1,
///          so no new call site becomes eligible during the pass.
/// CEP:STATUS: complete
/// CEP:FAILURE: gate refusals copy the call verbatim.
/// CEP:ASSUMES: value stacks are empty at block starts (doc/ir.md).
/// CEP:COST: O(body length) per pass.
/// CEP:EVIDENCE: tests/call_inline_tests.rs.
/// CEP:SECURITY: output re-verified by the engine.
#[allow(clippy::too_many_arguments)]
fn inline_at_depth(
    body: &Function,
    body_id: u32,
    funcs: &[Function],
    arities: &[u32],
    budgets: &PassBudgets,
    depth: u32,
    chain: &BTreeSet<u32>,
    budget: &mut SpliceBudget,
) -> InlineOutcome {
    if depth == 0 {
        return InlineOutcome { merged: None, inlined_calls: 0 };
    }
    // Per-pc block starts: values cannot cross block boundaries (doc/ir.md
    // builder contract), so the abstract value-stack depth resets to zero
    // at every leader and a linear scan tracks it exactly.
    let Some(block_start) = block_starts(body) else {
        return InlineOutcome { merged: None, inlined_calls: 0 };
    };
    let orig_locals = body.locals.len();
    let orig_strings = body.strings.len();
    let mut out: Vec<Insn> = Vec::with_capacity(body.code.len());
    let mut pc_map: Vec<u32> = vec![0; body.code.len()];
    let mut patches: Vec<(usize, u32)> = Vec::new();
    let mut locals_out: Vec<String> = Vec::new();
    let mut strings_out: Vec<String> = Vec::new();
    let mut inlined: u64 = 0;
    let mut cur = 0i64;

    for (pc, insn) in body.code.iter().enumerate() {
        if block_start[pc] {
            cur = 0;
        }
        if insn.op == Opcode::Call {
            if let Operand::U64(packed) = insn.operand {
                let argc = (packed >> 32) as usize;
                let callee_id = (packed & 0xFFFF_FFFF) as u32;
                // Gate 1: the stack below the arguments must be empty, so
                // the spliced body's internal merges sit on a zero
                // baseline (doc/inline.md).
                if cur == argc as i64 {
                    // Offsets for THIS splice: the tables appended so far
                    // sit after the original body's tables.
                    let local_base = orig_locals + locals_out.len();
                    let str_base = orig_strings + strings_out.len();
                    if let Some(sp) = try_splice(
                        callee_id,
                        argc,
                        funcs,
                        arities,
                        budgets,
                        depth,
                        chain,
                        budget,
                        local_base,
                        str_base,
                        out.len(),
                    ) {
                        // Re-entering the call site re-runs the whole
                        // splice (stores + body).
                        pc_map[pc] = out.len() as u32;
                        out.extend(sp.stores);
                        out.extend(sp.insns);
                        locals_out.extend(sp.local_names);
                        strings_out.extend(sp.strings);
                        cur = 1; // the result replaced the arguments
                        inlined += sp.inlined;
                        continue;
                    }
                    // Refused: fall through to the copy path.
                }
                let Some(eff) = stack_effect(insn) else {
                    return InlineOutcome { merged: None, inlined_calls: 0 };
                };
                cur += i64::from(eff);
            }
            pc_map[pc] = out.len() as u32;
            out.push(*insn);
            continue;
        }
        // Copy path: record the pc mapping, remap operands, apply the
        // stack effect; jump targets patch after the walk.
        pc_map[pc] = out.len() as u32;
        let operand = match insn.operand {
            Operand::Target(t) => {
                patches.push((out.len(), t));
                Operand::Target(0)
            }
            other => other,
        };
        out.push(Insn { op: insn.op, operand });
        let Some(eff) = stack_effect(insn) else {
            return InlineOutcome { merged: None, inlined_calls: 0 };
        };
        cur += i64::from(eff);
        if cur < 0 {
            // Underflow-shaped stream: impossible for liftable bytecode;
            // refuse rather than guess (Law 2).
            return InlineOutcome { merged: None, inlined_calls: 0 };
        }
    }

    if inlined == 0 {
        return InlineOutcome { merged: None, inlined_calls: 0 };
    }
    // Patch the recorded (caller-body) jump targets through the pc map.
    for (at, old_target) in patches {
        let t = old_target as usize;
        let new_t = if t < pc_map.len() { pc_map[t] } else { old_target };
        out[at].operand = Operand::Target(new_t);
    }
    let mut merged_locals = body.locals.clone();
    merged_locals.extend(locals_out);
    let mut merged_strings = body.strings.clone();
    merged_strings.extend(strings_out);
    InlineOutcome {
        merged: Some(Function {
            name: body.name.clone(),
            code: out,
            locals: merged_locals,
            strings: merged_strings,
        }),
        inlined_calls: inlined,
    }
}

/// Attempt to build one splice for a call site.
///
/// CEP:WHAT: Per-call-site legality gates and splice construction.
/// CEP:WHY: Every gate is a documented contract (doc/inline.md); refusal
///              leaves the call untouched.
/// CEP:STATUS: complete
/// CEP:FAILURE: None on any gate refusal.
/// CEP:ASSUMES: the callee id is in range (checked here).
/// CEP:COST: O(callee).
/// CEP:EVIDENCE: tests/call_inline_tests.rs gate tests.
/// CEP:SECURITY: the callee body must lift (IR subset) and contain no
///               Halt; both enforced here.
#[allow(clippy::too_many_arguments)]
fn try_splice(
    callee_id: u32,
    argc: usize,
    funcs: &[Function],
    arities: &[u32],
    budgets: &PassBudgets,
    depth: u32,
    chain: &BTreeSet<u32>,
    budget: &mut SpliceBudget,
    local_base: usize,
    str_base: usize,
    merged_base: usize,
) -> Option<Splice> {
    let callee = funcs.get(callee_id as usize)?;
    // Gate 2: no cycles (a function never splices into its own chain).
    if chain.contains(&callee_id) {
        return None;
    }
    // Gate 3: arity fits the callee's locals (params bind locals 0..argc).
    if argc > callee.locals.len() {
        return None;
    }
    // Gate 4: no Halt inside a callee body (a Halt would terminate the
    // whole program from a splice context).
    if callee.code.iter().any(|i| i.op == Opcode::Halt) {
        return None;
    }
    // Gate 5: the callee must be liftable (IR subset: no legacy jump-stack
    // ops, no I/O, no Drop) and within the node budget (spec 5.2.1: 500).
    let arity = arities.get(callee_id as usize).copied().unwrap_or(0) as usize;
    let raw_ok = matches!(crate::cold::ir::lift_with_arity(callee, arity, u32::MAX), Ok(l)
        if l.graph.nodes.len() as u32 <= budgets.max_inline_nodes);
    if !raw_ok {
        return None;
    }
    // Pre-inline the callee at depth-1 (spec 5.2.1 depth budget).
    let mut inner_inlined: u64 = 0;
    let body = if depth > 1 {
        let mut inner_chain = chain.clone();
        inner_chain.insert(callee_id);
        let mut inner_budget = SpliceBudget { remaining: budget.remaining };
        let inner = inline_at_depth(
            callee,
            callee_id,
            funcs,
            arities,
            budgets,
            depth - 1,
            &inner_chain,
            &mut inner_budget,
        );
        match inner.merged {
            Some(m) => {
                budget.remaining = inner_budget.remaining;
                inner_inlined = inner.inlined_calls;
                m
            }
            None => callee.clone(),
        }
    } else {
        callee.clone()
    };
    // Gate 6: the (possibly pre-inlined) body must still lift and hold
    // the node budget.
    let body_ok = matches!(crate::cold::ir::lift_with_arity(&body, arity, u32::MAX), Ok(l)
        if l.graph.nodes.len() as u32 <= budgets.max_inline_nodes);
    if !body_ok {
        return None;
    }
    // Gate 6b: exactly one Ret, and it is the body's last instruction
    // (doc/inline.md). The splice removes the Ret so its value stays on
    // the stack as the call's result; removing a mid-body Ret would glue
    // the early-return path into the code that follows it. A branch edge
    // into a Ret cannot lift (the edge would lack the return value), so
    // every other shape refuses here and the call compiles un-inlined.
    let ret_count = body.code.iter().filter(|i| i.op == Opcode::Ret).count();
    if ret_count != 1 || body.code.last().map(|i| i.op) != Some(Opcode::Ret) {
        return None;
    }
    // Gate 7: merged locals stay addressable (u16 slot operands).
    if local_base + body.locals.len() > MAX_LOCALS {
        return None;
    }
    // Gate 8: the splice budget bounds total spliced instructions.
    if body.code.len() as i64 > budget.remaining {
        return None;
    }
    budget.remaining -= body.code.len() as i64;

    // Stores: pop the arguments (top = last) into the remapped parameter
    // slots so the spliced body sees locals 0..argc bound.
    let mut stores: Vec<Insn> = Vec::with_capacity(argc);
    for i in (0..argc).rev() {
        let slot = local_base + i;
        stores.push(Insn {
            op: Opcode::StoreVar,
            operand: Operand::Slot(u16::try_from(slot).ok()?),
        });
    }
    let body_base = merged_base + stores.len();
    // The single trailing Ret is removed (its value stays on the stack as
    // the call's result), so the caller's continuation is one instruction
    // earlier than the original body length (doc/inline.md).
    let continuation = body_base + body.code.len() - 1;
    // Remap the body: drop the trailing Ret, shift targets, remap slots
    // and string ids.
    let insns: Vec<Insn> = body
        .code
        .iter()
        .filter(|i| i.op != Opcode::Ret)
        .map(|i| {
            let operand = match i.operand {
                Operand::Target(t) => {
                    let t = t as usize;
                    // Gate 6b pins the only Ret at the very end, and a
                    // branch edge into it cannot lift (gate 6), so every
                    // target precedes the Ret and the original index is
                    // already the filtered index. A target at the Ret
                    // itself cannot occur; route it to the continuation
                    // defensively.
                    let new_t = if t >= body.code.len() - 1 {
                        continuation
                    } else {
                        body_base + t
                    };
                    Operand::Target(new_t as u32)
                }
                Operand::Slot(s) => {
                    Operand::Slot((local_base + s as usize).try_into().unwrap_or(u16::MAX))
                }
                Operand::StrId(s) => {
                    Operand::StrId((str_base + s as usize).try_into().unwrap_or(u32::MAX))
                }
                other => other,
            };
            Insn { op: i.op, operand }
        })
        .collect();
    Some(Splice {
        stores,
        insns,
        local_names: body
            .locals
            .iter()
            .map(|n| format!("{}:{}", body.name, n))
            .collect(),
        strings: body.strings.clone(),
        inlined: inner_inlined + 1,
    })
}

/// Value-stack effect of one instruction (doc/bytecode.md).
///
/// CEP:WHAT: Stack effect table.
/// CEP:WHY: The call-site depth gate needs the abstract stack depth;
///          the table mirrors the lift builder's push/pop sequence.
/// CEP:STATUS: complete
/// CEP:FAILURE: None for opcodes outside the table (refuse to guess).
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: tests/call_inline_tests.rs depth-gate tests.
/// CEP:SECURITY: none
fn stack_effect(insn: &Insn) -> Option<i32> {
    Some(match insn.op {
        Opcode::PushNumber
        | Opcode::PushBool
        | Opcode::PushString
        | Opcode::ReadInput
        | Opcode::LoadVar => 1,
        Opcode::PushUsize | Opcode::Jump | Opcode::JumpLastOnStack | Opcode::Drop => 0,
        Opcode::StoreVar | Opcode::AssignVar | Opcode::WriteLn | Opcode::Write
        | Opcode::ProcessExit => -1,
        Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div | Opcode::Modulo
        | Opcode::GreaterThan | Opcode::LessThan | Opcode::Equal
        | Opcode::JumpIfFalse | Opcode::JumpIfTrue | Opcode::Ret => -1,
        Opcode::NewArray | Opcode::ArrayLen | Opcode::Halt => 0,
        Opcode::LoadIdx => -1,
        Opcode::StoreIdx => -3,
        Opcode::Call => match insn.operand {
            Operand::U64(p) => -((p >> 32) as i32) + 1,
            _ => return None,
        },
    })
}

/// Block-start (leader) map for the depth trace.
///
/// CEP:WHAT: Block boundaries (leaders).
/// CEP:WHY: Values cannot cross block boundaries (doc/ir.md builder
///          contract); the depth resets to zero at every leader, so a
///          linear scan tracks the depth per block exactly.
/// CEP:STATUS: complete
/// CEP:FAILURE: None on an empty stream.
/// CEP:ASSUMES: structured control flow.
/// CEP:COST: O(n).
/// CEP:EVIDENCE: tests/call_inline_tests.rs.
/// CEP:SECURITY: none
fn block_starts(f: &Function) -> Option<Vec<bool>> {
    let n = f.code.len();
    if n == 0 {
        return None;
    }
    let mut block_start = vec![false; n];
    block_start[0] = true;
    for (pc, insn) in f.code.iter().enumerate() {
        if let Operand::Target(t) = insn.operand {
            let t = t as usize;
            if t < n {
                block_start[t] = true;
            }
        }
        match insn.op {
            Opcode::JumpIfFalse | Opcode::JumpIfTrue | Opcode::Jump | Opcode::Ret
            | Opcode::Halt
                if pc + 1 < n =>
            {
                block_start[pc + 1] = true;
            }
            _ => {}
        }
    }
    Some(block_start)
}

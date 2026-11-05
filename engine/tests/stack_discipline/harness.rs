// CEP:FILE: engine/tests/stack_discipline/harness.rs
// CEP:WHAT: The matrix test harness: the symbolic instruction set, the
//           two-pass assembler, the tier runner, the parity assertion and
//           the shared loop/diamond program builders.
// CEP:WHY: Every matrix case assembles and runs through these helpers, so
//          a harness defect shows as mass divergence instead of a false
//          pass (doc/differential.md).
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: the assembler panics on unresolvable labels (authoring
//              bug, loud); assert_parity fails with the case name, the
//              tier and both values.
// CEP:ASSUMES: f32-exact constants (doc/differential.md); interpreter
//              Halt returns the bottom of the value stack, so every case
//              keeps at least one value on the stack at Halt.
// CEP:COST: offline.
// CEP:EVIDENCE: every matrix case routes through run_tier/assert_parity.
// CEP:SECURITY: test-authored bytecode only.

use vertex_jit::cold::bytecode::decode_vertex;
use vertex_jit::cold::engine::JitEngine;
use vertex_jit::cold::errors::EngineError;
use vertex_jit::cold::tier::{JitBudgets, Tier};
use vertex_jit::hot::host::HostHooks;
use vertex_jit::hot::value::Value;
use vertex_jit::platform::TargetConfig;

/// Symbolic instruction for the test assembler (labels resolved in a
/// second pass; variable names resolved by `decode_vertex` first-use
/// order, exactly like the production front end).
#[derive(Clone, Copy)]

pub(crate) enum I {
    /// Push an f64 constant (assembled as f32, decode widens).
    Push(f32),
    /// Push a boolean.
    PushBool(bool),
    Add,
    Sub,
    Mul,
    Lt,
    Gt,
    Eq,
    Store(&'static str),
    Load(&'static str),
    Assign(&'static str),
    /// Binding retirement; never touches the value stack.
    Drop(&'static str),
    Jump(u16),
    JumpIfFalse(u16),
    JumpIfTrue(u16),
    /// Symbolic label; resolved to a pc by the assembler.
    L(u16),
    Halt,
}

/// Assemble symbolic instructions to legacy bytecode bytes.
///
/// CEP:WHAT: Test bytecode assembler.
/// CEP:WHY: Matrices need exact programs without hand-counted pcs.
/// CEP:STATUS: complete
/// CEP:FAILURE: Panics on unresolvable labels (authoring bug, loud).
/// CEP:ASSUMES: label ids unique per program.
/// CEP:COST: O(n) two passes.
/// CEP:EVIDENCE: every matrix case assembles through it.
/// CEP:SECURITY: test-authored constants only.
pub(crate) fn asm(code: &[I]) -> Vec<u8> {
    // Pass 1: labels map to INSTRUCTION indices (the legacy jump-target
    // convention; verify() bounds targets by code.len(), not by bytes).
    let mut label_pc = vec![0u16; 256];
    let mut idx = 0usize;
    for insn in code {
        match insn {
            I::L(l) => label_pc[*l as usize] = idx as u16,
            _ => idx += 1,
        }
    }
    // Pass 2: emit.
    let mut out = Vec::with_capacity(256);
    for insn in code {
        match *insn {
            I::L(_) => {}
            I::Push(v) => {
                out.push(22);
                out.extend_from_slice(&v.to_le_bytes());
            }
            I::PushBool(b) => {
                out.push(21);
                out.push(b as u8);
            }
            I::Add => out.push(1),
            I::Sub => out.push(2),
            I::Mul => out.push(4),
            I::Lt => out.push(7),
            I::Gt => out.push(6),
            I::Eq => out.push(8),
            I::Store(n) => emit_name(&mut out, 50, n),
            I::Load(n) => emit_name(&mut out, 51, n),
            I::Assign(n) => emit_name(&mut out, 52, n),
            I::Drop(n) => emit_name(&mut out, 125, n),
            I::Jump(l) => {
                out.push(40);
                out.extend_from_slice(&label_pc[l as usize].to_le_bytes());
            }
            I::JumpIfFalse(l) => {
                out.push(41);
                out.extend_from_slice(&label_pc[l as usize].to_le_bytes());
            }
            I::JumpIfTrue(l) => {
                out.push(42);
                out.extend_from_slice(&label_pc[l as usize].to_le_bytes());
            }
            I::Halt => out.push(255),
        }
    }
    out
}

/// CEP:WHAT: Name-operand instruction emitter (u32 length + UTF-8 name).
/// CEP:WHY: Store/Load/Assign carry names; the legacy bytecode encodes them as
/// u32-length-prefixed bytes, exactly like the production decoder.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics never; byte length derives from the name.
/// CEP:ASSUMES: names are static test strings.
/// CEP:COST: O(name length).
/// CEP:EVIDENCE: every Store/Load/Assign case assembles through it.
fn emit_name(out: &mut Vec<u8>, op: u8, name: &str) {
    out.push(op);
    out.extend_from_slice(&(name.len() as u32).to_le_bytes());
    out.extend_from_slice(name.as_bytes());
}

/// CEP:WHAT: Run one assembled program on one tier.
/// CEP:WHY: Matrix parity needs each tier exercised in isolation with the same
/// engine construction the production path uses.
/// CEP:STATUS: complete
/// CEP:FAILURE: returns EngineError from decode or engine construction; the
///              caller panics on unexpected errors.
/// CEP:ASSUMES: TargetConfig::detect() succeeds on the test host.
/// CEP:COST: one engine build plus one run per call.
/// CEP:EVIDENCE: assert_parity and every matrix test route through it.
pub(crate) fn run_tier(name: &str, bytes: &[u8], tier: Tier) -> Result<Option<Value>, EngineError> {
    let func = decode_vertex(name, bytes)?;
    let mut e = JitEngine::new(vec![func], &TargetConfig::detect()?, JitBudgets::default())?;
    e.tiers[0] = tier;
    let exit = e.run(0, &HostHooks::stdio())?;
    Ok(match exit {
        vertex_jit::hot::interp::Exit::Halt(v) => v,
        vertex_jit::hot::interp::Exit::Returned(v) => Some(v),
        vertex_jit::hot::interp::Exit::ProcessExited(code) => Some(Value::Number(code)),
        vertex_jit::hot::interp::Exit::BackEdgeBudget { .. } => None,
    })
}

/// CEP:WHAT: Differential parity assertion across Interp/T0/T1/T2/T3.
/// CEP:WHY: Standard 61.3: a matrix case passes only when every tier matches the
/// interpreter bit-for-bit.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics naming the case, the diverging tier and both values.
/// CEP:ASSUMES: the interpreter result is the reference semantics.
/// CEP:COST: five runs per case.
/// CEP:EVIDENCE: matrices A/B/C drive it for all 150 cases.
pub(crate) fn assert_parity(name: &str, bytes: &[u8]) {
    let interp = run_tier(name, bytes, Tier::Interp).expect("interp run");
    eprintln!("case {name}: interp -> {interp:?}");
    for tier in [Tier::T0, Tier::T1, Tier::T2, Tier::T3] {
        let got = run_tier(name, bytes, tier)
            .unwrap_or_else(|e| panic!("{name} tier {tier:?} failed: {e}"));
        eprintln!("case {name}: {tier:?} -> {got:?}");
        assert_eq!(
            interp, got,
            "{name}: tier {tier:?} diverged from the interpreter"
        );
    }
}

/// CEP:WHAT: Counted down-to loop builder with the compare in the loop head.
/// CEP:WHY: Matrix shapes need the compare-in-head form because that is where the
/// compare templates executed once per iteration (defect class A).
/// CEP:STATUS: complete
/// CEP:FAILURE: none (pure program construction).
/// CEP:ASSUMES: labels L(0)/L(1) are used only inside the built program.
/// CEP:COST: O(body length).
/// CEP:EVIDENCE: matrices A/B/C build their loop cases with it.
pub(crate) fn while_lt(init: f32, bound: f32, body: &[I]) -> Vec<I> {
    let mut v = vec![I::Push(init), I::Store("i")];
    v.push(I::L(0)); // head
    v.push(I::Load("i"));
    v.push(I::Push(bound));
    v.push(I::Lt);
    v.push(I::JumpIfFalse(1)); // exit
    v.extend_from_slice(body);
    v.push(I::Jump(0));
    v.push(I::L(1)); // exit
    v.push(I::Load("i"));
    v.push(I::Halt);
    v
}

/// CEP:WHAT: Diamond builder: if (a OP b) c = x else c = y, then the tail.
/// CEP:WHY: Environment snapshots at branch time (defect class C) need a shape
/// where both arms define the join value exactly once.
/// CEP:STATUS: complete
/// CEP:FAILURE: none (pure program construction).
/// CEP:ASSUMES: each arm stores `c` exactly once (T0 subset discipline).
/// CEP:COST: O(tail length).
/// CEP:EVIDENCE: matrix C diamond cases build through it.
pub(crate) fn diamond(a: f32, op: I, b: f32, x: f32, y: f32, tail: &[I]) -> Vec<I> {
    vec![
        I::Push(a),
        I::Push(b),
        op,
        I::JumpIfFalse(1),
        I::Push(x),
        I::Store("c"),
        I::Jump(2),
        I::L(1),
        I::Push(y),
        I::Store("c"),
        I::L(2),
    ]
    .into_iter()
    .chain(tail.iter().copied())
    .collect()
}

/// CEP:WHAT: Counted down-to loop builder with the greater-than compare in the head.
/// CEP:WHY: Defect class A needs the Gt template exercised in loop heads the same
/// way Lt is.
/// CEP:STATUS: complete
/// CEP:FAILURE: none (pure program construction).
/// CEP:ASSUMES: labels L(0)/L(1) are used only inside the built program.
/// CEP:COST: O(body length).
/// CEP:EVIDENCE: matrix A boundary/context cases build through it.
pub(crate) fn while_gt(init: f32, bound: f32, body: &[I]) -> Vec<I> {
    let mut v = vec![I::Push(init), I::Store("i")];
    v.push(I::L(0));
    v.push(I::Load("i"));
    v.push(I::Push(bound));
    v.push(I::Gt);
    v.push(I::JumpIfFalse(1));
    v.extend_from_slice(body);
    v.push(I::Jump(0));
    v.push(I::L(1));
    v.push(I::Load("i"));
    v.push(I::Halt);
    v
}

/// CEP:WHAT: Repeat-until-equal loop builder with the compare in the body.
/// CEP:WHY: Defect classes A and C need the body-compare form (JumpIfTrue exit)
/// to exercise the back-edge environment differently.
/// CEP:STATUS: complete
/// CEP:FAILURE: none (pure program construction).
/// CEP:ASSUMES: the step makes the loop terminate.
/// CEP:COST: O(1) construction.
/// CEP:EVIDENCE: matrix A/B loop cases build through it.
pub(crate) fn until_eq(init: f32, step: f32, bound: f32) -> Vec<I> {
    vec![
        I::Push(init),
        I::Store("i"),
        I::L(0), // head
        I::Load("i"),
        I::Push(step),
        I::Add,
        I::Assign("i"),
        I::Load("i"),
        I::Push(bound),
        I::Eq,
        I::JumpIfTrue(1), // exit
        I::Jump(0),
        I::L(1),
        I::Load("i"),
        I::Halt,
    ]
}

/// CEP:WHAT: Minimal counted loop whose program ends right after the loop body.
/// CEP:WHY: Drop-class cases (matrix B) append their own halt tails, so the
/// builder must expose the body-last form without the default tail.
/// CEP:STATUS: complete
/// CEP:FAILURE: none (pure program construction).
/// CEP:ASSUMES: the caller appends a valid tail.
/// CEP:COST: O(1) construction.
/// CEP:EVIDENCE: matrix B trigger and boundary cases build through it.
pub(crate) fn while_lt_body_last(init: f32, bound: f32) -> Vec<I> {
    while_lt(init, bound, INC)
}

pub(crate) const INC: &[I] = &[
    I::Load("i"),
    I::Push(1.0),
    I::Add,
    I::Assign("i"),
];

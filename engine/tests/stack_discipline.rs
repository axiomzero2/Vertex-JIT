// CEP:FILE: engine/tests/stack_discipline.rs
// CEP:WHAT: 50-test regression matrices for two confirmed defect classes:
//           (a) the comparison templates (LessThan, GreaterThan, Equal)
//           pushed their boolean result instead of collapsing the operand
//           pair in place, leaking 16 machine-stack bytes per execution
//           and overflowing the stack inside loops; (b) the T0 depth
//           simulation popped the value stack on Drop(slot) although the
//           machine template, the interpreter and check_legality treat
//           Drop as a binding retirement with no value-stack effect, so
//           the halt tail read the wrong slot.
// CEP:WHY: Standard 61.3: a fix is not understood until 50 variations are
//           proven safe; a single reproducing test proves only the exact
//           input. Each matrix is 1 trigger + 5 boundary + 10 type/size +
//           14 context + 20 negative.
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: Any tier diverging from the interpreter fails with the case
//              name, the tier and both values.
// CEP:ASSUMES: f32-exact constants (doc/differential.md); interpreter Halt
//              returns the bottom of the value stack, so every case keeps
//              at least one value on the stack at Halt.
// CEP:COST: offline.
// CEP:EVIDENCE: this file; the trigger cases reproduce the pre-fix
//               failures (loop stack overflow; Drop returning 0 for 7).
// CEP:TEST-CLASS: Regression Matrix
// CEP:TEST-TARGET: T0 template emitter (compare/equal); compile() halt-tail
//                  depth simulation (Drop arm).
// CEP:TEST-QUOTA: 50 tests per defect class (1 trigger, 5 boundary, 10
//                 type/size, 14 context, 20 negative).
// CEP:TEST-DISTRIBUTION: worst-case deep and large-trip loops (stress);
//                 middle contextual loop and branch shapes; straight-line
//                 negatives prove the fix changed no correct program.
// CEP:TEST-SCALING: base 50 per class; one more matrix per newly confirmed
//                 template or depth-simulation defect class.
// CEP:TEST-EVIDENCE: this file, differential parity on Interp/T0/T1/T2/T3.
// CEP:SECURITY: test-authored bytecode only; no host input.
//! Regression matrices for the T0 stack-discipline defect classes
//! (standard 61.3: fifty tests per confirmed bug).

use vertex_jit::cold::bytecode::decode_vertex;
use vertex_jit::cold::engine::JitEngine;
use vertex_jit::cold::errors::EngineError;
use vertex_jit::cold::tier::{JitBudgets, Tier};
use vertex_jit::hot::interp::HostHooks;
use vertex_jit::hot::value::Value;
use vertex_jit::platform::TargetConfig;

/// Symbolic instruction for the test assembler (labels resolved in a
/// second pass; variable names resolved by `decode_vertex` first-use
/// order, exactly like the production front end).
#[derive(Clone, Copy)]
enum I {
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
fn asm(code: &[I]) -> Vec<u8> {
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

/// Emit a name-operand instruction (u32 length + UTF-8 name).
fn emit_name(out: &mut Vec<u8>, op: u8, name: &str) {
    out.push(op);
    out.extend_from_slice(&(name.len() as u32).to_le_bytes());
    out.extend_from_slice(name.as_bytes());
}

/// Run one program on one tier and return the exit value.
fn run_tier(name: &str, bytes: &[u8], tier: Tier) -> Result<Option<Value>, EngineError> {
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

/// Differential assertion across the interpreter and all four tiers.
fn assert_parity(name: &str, bytes: &[u8]) {
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

/// `i = init; while i < bound { body }; return i` with a counter compare in
/// the loop head (the shape that executed the compare once per iteration).
fn while_lt(init: f32, bound: f32, body: &[I]) -> Vec<I> {
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

/// Diamond: `if (a OP b) { c = x } else { c = y }; <tail>`. Each arm stores
/// exactly once (the T0 subset forbids re-binding), the join reads `c`.
fn diamond(a: f32, op: I, b: f32, x: f32, y: f32, tail: &[I]) -> Vec<I> {
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

/// `i = init; while i > bound { body }; return i`.
fn while_gt(init: f32, bound: f32, body: &[I]) -> Vec<I> {
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

/// `i = init; loop { i += step; if i == bound break }; return i` with the
/// equality compare inside the body (JumpIfTrue exit).
fn until_eq(init: f32, step: f32, bound: f32) -> Vec<I> {
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

const INC: &[I] = &[
    I::Load("i"),
    I::Push(1.0),
    I::Add,
    I::Assign("i"),
];

/// Matrix A: comparison stack discipline (50 cases).
fn matrix_a() -> Vec<(&'static str, Vec<I>)> {
    let mut v: Vec<(&'static str, Vec<I>)> = Vec::new();
    // Trigger (1): the exact failing program shape; pre-fix this overflowed
    // the machine stack (compare leak 16 bytes/iteration).
    v.push(("A01_trigger_while_lt_3", while_lt(0.0, 3.0, INC)));
    // Boundary (5): trip counts 0, 1, 2, 3 and a deep stress trip.
    v.push(("A02_boundary_trip_0", while_lt(0.0, 0.0, INC)));
    v.push(("A03_boundary_trip_1", while_lt(0.0, 1.0, INC)));
    v.push(("A04_boundary_trip_2", while_lt(0.0, 2.0, INC)));
    v.push(("A05_boundary_trip_3", while_lt(0.0, 3.0, INC)));
    v.push(("A06_boundary_trip_1000", while_lt(0.0, 1000.0, INC)));
    // Type/size (10): every comparison opcode across operand classes.
    v.push(("A07_lt_negative_start", while_lt(-2.0, 2.0, INC)));
    v.push((
        "A08_gt_countdown",
        while_gt(5.0, 0.0, &[I::Load("i"), I::Push(1.0), I::Sub, I::Assign("i")]),
    ));
    v.push(("A09_eq_exit_body", until_eq(0.0, 1.0, 5.0)));
    v.push(("A10_eq_fractional_step", until_eq(0.0, 0.25, 2.0)));
    v.push((
        "A11_lt_constant_left",
        vec![
            I::Push(3.0),
            I::Store("i"),
            I::L(0),
            I::Push(0.0),
            I::Load("i"),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(1.0),
            I::Sub,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ],
    ));
    v.push(("A12_lt_large_values", while_lt(0.0, 100000.0, &[I::Load("i"), I::Push(25000.0), I::Add, I::Assign("i")])));
    v.push((
        "A13_two_local_compare",
        vec![
            I::Push(0.0),
            I::Store("a"),
            I::Push(3.0),
            I::Store("b"),
            I::L(0),
            I::Load("a"),
            I::Load("b"),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("a"),
            I::Push(1.0),
            I::Add,
            I::Assign("a"),
            I::Jump(0),
            I::L(1),
            I::Load("a"),
            I::Halt,
        ],
    ));
    v.push((
        "A14_compare_computed_left",
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Push(6.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ],
    ));
    v.push(("A15_eq_large_steps", until_eq(0.0, 1000.0, 4000.0)));
    v.push((
        "A16_gt_negative_bound",
        while_gt(2.0, -2.0, &[I::Load("i"), I::Push(1.0), I::Sub, I::Assign("i")]),
    ));
    // Context (14).
    v.push(("A17_jumpiftrue_guarded_head", {
        // while !(i == 4) { i += 1 } expressed as an Eq head with
        // JumpIfTrue exit (inverted branch polarity).
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(4.0),
            I::Eq,
            I::JumpIfTrue(1),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("A18_two_sequential_loops", {
        let mut p = while_lt_body_last(0.0, 2.0);
        p.pop(); // Halt
        p.pop(); // exit Load i
        p.extend([
            I::L(2),
            I::Load("i"),
            I::Push(0.0),
            I::Gt,
            I::JumpIfFalse(3),
            I::Load("i"),
            I::Push(1.0),
            I::Sub,
            I::Assign("i"),
            I::Jump(2),
            I::L(3),
            I::Load("i"),
            I::Halt,
        ]);
        p
    }));
    v.push(("A19_two_compares_per_iteration", {
        // while i < 6 { if i > 3 { i += 2 } else { i += 1 } } -> 6
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(6.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(3.0),
            I::Gt,
            I::JumpIfFalse(2), // else
            I::Load("i"),
            I::Push(2.0),
            I::Add,
            I::Assign("i"),
            I::Jump(3), // join
            I::L(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::L(3),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("A20_nested_loop_inner_compare", {
        // j = 0; while j < 2 { i = 0; while i < 3 { i += 1 }; j += 1 }
        vec![
            I::Push(0.0),
            I::Store("j"),
            I::L(0),
            I::Load("j"),
            I::Push(2.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Push(0.0),
            I::Store("i"),
            I::L(2),
            I::Load("i"),
            I::Push(3.0),
            I::Lt,
            I::JumpIfFalse(3),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(2),
            I::L(3),
            I::Load("j"),
            I::Push(1.0),
            I::Add,
            I::Assign("j"),
            I::Jump(0),
            I::L(1),
            I::Load("j"),
            I::Halt,
        ]
    }));
    v.push(("A21_compare_result_stored", {
        // r = false; i = 0; while i < 3 { r = (i < 10); i += 1 }; return r
        vec![
            I::PushBool(false),
            I::Store("r"),
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(3.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(10.0),
            I::Lt,
            I::Assign("r"),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("r"),
            I::Halt,
        ]
    }));
    v.push(("A22_do_until_gt", {
        // loop { i += 1; if i > 3 break } -> 4
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Load("i"),
            I::Push(3.0),
            I::Gt,
            I::JumpIfTrue(1),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("A23_self_compare_immediate_exit", {
        // i = 5; while i < i { i += 1 } -> 5 (compare taken on first eval)
        vec![
            I::Push(5.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Load("i"),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push((
        "A24_gt_negative_step",
        while_gt(10.0, 0.0, &[I::Load("i"), I::Push(-1.0), I::Add, I::Assign("i")]),
    ));
    v.push(("A25_compare_in_loop_drop_result_after", {
        // r = false; i = 0; while i < 3 { r = (i < 9); i += 1 };
        // drop r; return i. Cross-class context: the comparison runs per
        // iteration and the compare-result slot is retired afterwards.
        // A Drop cannot execute twice in a legal T0 program (bindings do
        // not re-bind), so the Drop lives after the loop.
        vec![
            I::PushBool(false),
            I::Store("r"),
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(3.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(9.0),
            I::Lt,
            I::Assign("r"),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Drop("r"),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("A26_branch_polarity_both_ways", {
        // while i < 8 { if i < 4 { i += 1 } else { i += 2 } } -> 8
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(8.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(4.0),
            I::Lt,
            I::JumpIfFalse(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(3),
            I::L(2),
            I::Load("i"),
            I::Push(2.0),
            I::Add,
            I::Assign("i"),
            I::L(3),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("A27_compare_after_loop", {
        // while i < 2 { i += 1 }; if i == 2 { i = 9 } -> 9. The assignment
        // consumes its own Push; leaving the compared Load on the stack
        // would make the interpreter's bottom-of-stack Halt return the
        // stray value while the IR tiers drop it (unbalanced shape).
        let mut p = while_lt_body_last(0.0, 2.0);
        p.pop();
        p.pop();
        p.extend([
            I::Load("i"),
            I::Push(2.0),
            I::Eq,
            I::JumpIfFalse(4),
            I::Push(9.0),
            I::Assign("i"),
            I::L(4),
            I::Load("i"),
            I::Halt,
        ]);
        p
    }));
    v.push(("A28_joint_exit_two_counters", {
        // i = 0; j = 0; while i < 10 { j += 2; if j == 6 break; i += 1 }
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::Push(0.0),
            I::Store("j"),
            I::L(0),
            I::Load("i"),
            I::Push(10.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("j"),
            I::Push(2.0),
            I::Add,
            I::Assign("j"),
            I::Load("j"),
            I::Push(6.0),
            I::Eq,
            I::JumpIfTrue(1),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("A29_condition_uses_prior_loop_result", {
        // while i < 2 { i += 1 }; while j < i { j += 1 } -> 2
        let mut p = while_lt_body_last(0.0, 2.0);
        p.pop();
        p.pop();
        p.extend([
            I::Push(0.0),
            I::Store("j"),
            I::L(5),
            I::Load("j"),
            I::Load("i"),
            I::Lt,
            I::JumpIfFalse(6),
            I::Load("j"),
            I::Push(1.0),
            I::Add,
            I::Assign("j"),
            I::Jump(5),
            I::L(6),
            I::Load("j"),
            I::Halt,
        ]);
        p
    }));
    // Deep stress: 19000 trips, just inside the interpreter's single OSR
    // budget window (TierThresholds::osr_at = 20000 back-edges; a larger
    // trip count makes the interpreter return BackEdgeBudget, which is a
    // tiering handoff, not a value). Pre-fix the comparison templates
    // leaked 16 bytes per iteration: 304 KB of machine-stack growth.
    v.push(("A30_deep_stress_trip_19000", while_lt(0.0, 19000.0, INC)));
    // Negative (20): straight-line comparisons and comparison-free loops;
    // the fix must not change any correct program.
    v.push(("A31_straight_lt_true", vec![I::Push(2.0), I::Push(5.0), I::Lt, I::Halt]));
    v.push(("A32_straight_lt_false", vec![I::Push(5.0), I::Push(2.0), I::Lt, I::Halt]));
    v.push(("A33_straight_gt_false", vec![I::Push(2.0), I::Push(5.0), I::Gt, I::Halt]));
    v.push(("A34_straight_gt_equal", vec![I::Push(5.0), I::Push(5.0), I::Gt, I::Halt]));
    v.push(("A35_straight_eq_true", vec![I::Push(5.0), I::Push(5.0), I::Eq, I::Halt]));
    v.push(("A36_straight_eq_false", vec![I::Push(5.0), I::Push(6.0), I::Eq, I::Halt]));
    v.push(("A37_straight_lt_negatives", vec![I::Push(-3.0), I::Push(2.0), I::Lt, I::Halt]));
    v.push(("A38_straight_gt_mixed_sign", vec![I::Push(2.0), I::Push(-3.0), I::Gt, I::Halt]));
    v.push(("A39_straight_eq_zeros", vec![I::Push(0.0), I::Push(0.0), I::Eq, I::Halt]));
    v.push(("A40_straight_lt_fractions", vec![I::Push(0.25), I::Push(0.25), I::Lt, I::Halt]));
    v.push(("A41_straight_arithmetic_only", vec![I::Push(6.0), I::Push(7.0), I::Add, I::Push(2.0), I::Mul, I::Halt]));
    v.push(("A42_branch_no_loop", vec![
        I::PushBool(true),
        I::JumpIfFalse(1),
        I::Push(3.0),
        I::Jump(2),
        I::L(1),
        I::Push(0.0),
        I::L(2),
        I::Halt,
    ]));
    v.push(("A43_compare_once_before_loop", {
        // r = (3 < 5); i = 0; while i < 2 { i += 1 }; return r. The
        // comparison executes once while the loop it precedes is compiled
        // and executed in the same function.
        let mut p = vec![I::Push(3.0), I::Push(5.0), I::Lt, I::Store("r")];
        p.extend(while_lt(0.0, 2.0, INC));
        p.pop(); // Halt
        p.pop(); // exit Load i
        p.push(I::Load("r"));
        p.push(I::Halt);
        p
    }));
    v.push(("A44_jumpiftrue_straight", vec![
        I::PushBool(true),
        I::JumpIfTrue(1),
        I::Push(0.0),
        I::Jump(2),
        I::L(1),
        I::Push(7.0),
        I::L(2),
        I::Halt,
    ]));
    v.push(("A45_compare_chain_bottom", vec![
        I::Push(1.0), I::Push(2.0), I::Lt,
        I::Push(3.0), I::Push(4.0), I::Gt,
        I::Halt,
    ]));
    v.push(("A46_compare_feeds_branch_straight", vec![
        I::Push(2.0), I::Push(3.0), I::Lt,
        I::JumpIfFalse(1),
        I::Push(10.0),
        I::Jump(2),
        I::L(1),
        I::Push(20.0),
        I::L(2),
        I::Halt,
    ]));
    v.push(("A47_compare_of_computed_straight", vec![
        I::Push(4.0), I::Push(1.0), I::Add, I::Push(5.0), I::Eq, I::Halt,
    ]));
    v.push(("A48_loop_result_unused", {
        // b = 1; while i < 3 { i += 1 }; return b (comparison in loop, the
        // returned value independent of it).
        let mut p = vec![I::Push(1.0), I::Store("b")];
        p.extend(while_lt(0.0, 3.0, INC));
        p.pop(); // Halt
        p.pop(); // exit Load i
        p.push(I::Load("b"));
        p.push(I::Halt);
        p
    }));
    v.push(("A49_gt_odd_step_nonboundary_exit", while_gt(9.0, 4.0, &[I::Load("i"), I::Push(2.0), I::Sub, I::Assign("i")])));
    v.push(("A50_deep_stack_at_halt_with_compares", vec![
        I::Push(1.0), I::Push(2.0), I::Lt,
        I::Push(3.0), I::Push(4.0), I::Gt,
        I::Push(5.0), I::Push(6.0), I::Eq,
        I::Halt,
    ]));
    v
}

/// `i = init; while i < bound { i += 1 }; <tail>`; the caller pops the
/// default `Load i; Halt` tail when it needs a custom ending.
fn while_lt_body_last(init: f32, bound: f32) -> Vec<I> {
    while_lt(init, bound, INC)
}

/// Matrix B: Drop depth-simulation (50 cases). The interpreter and
/// check_legality treat Drop(slot) as a binding retirement only; the T0
/// depth simulation must agree or the halt tail reads the wrong slot.
fn matrix_b() -> Vec<(&'static str, Vec<I>)> {
    let mut v: Vec<(&'static str, Vec<I>)> = Vec::new();
    // Trigger (1): pre-fix the depth simulation popped on Drop, the halt
    // tail saw depth 0 and returned 0 instead of 7.
    v.push(("B01_trigger_drop_before_halt", vec![
        I::Push(5.0), I::Store("i"), I::Push(7.0), I::Drop("i"), I::Halt,
    ]));
    // Boundary (5): Drop at value-stack depths 0 through 3 and the empty
    // slot region boundary.
    v.push(("B02_drop_depth1_push_after", vec![
        I::Push(1.0), I::Store("i"), I::Drop("i"), I::Push(2.0), I::Halt,
    ]));
    v.push(("B03_drop_depth2_halt_depth2", vec![
        I::Push(1.0), I::Push(2.0), I::Store("i"), I::Drop("i"), I::Halt,
    ]));
    v.push(("B04_drop_then_single_push", vec![
        I::Push(1.0), I::Store("i"), I::Drop("i"), I::Push(9.0), I::Halt,
    ]));
    v.push(("B05_drop_one_of_two_slots", vec![
        I::Push(3.0), I::Store("i"), I::Push(4.0), I::Store("j"),
        I::Drop("i"), I::Load("j"), I::Halt,
    ]));
    v.push(("B06_drop_both_slots", vec![
        I::Push(3.0), I::Store("i"), I::Push(4.0), I::Store("j"),
        I::Drop("i"), I::Drop("j"), I::Push(5.0), I::Halt,
    ]));
    // Type/size (10): slot counts, name lengths, lattice types, value
    // shapes around the Drop.
    v.push(("B07_drop_bool_slot", vec![
        I::PushBool(true), I::Store("b"), I::Push(6.0), I::Drop("b"), I::Halt,
    ]));
    v.push(("B08_three_slots_two_drops", vec![
        I::Push(1.0), I::Store("a"), I::Push(2.0), I::Store("b"),
        I::Push(3.0), I::Store("c"), I::Drop("a"), I::Drop("c"),
        I::Push(8.0), I::Halt,
    ]));
    v.push(("B09_long_slot_name", vec![
        I::Push(2.0), I::Store("variable_with_long_name_0123456789"),
        I::Push(4.0), I::Drop("variable_with_long_name_0123456789"), I::Halt,
    ]));
    v.push(("B10_ten_slots_all_dropped", {
        let mut p = Vec::new();
        for k in 0..10 {
            p.push(I::Push(k as f32));
            p.push(I::Store(match k {
                0 => "v0", 1 => "v1", 2 => "v2", 3 => "v3", 4 => "v4",
                5 => "v5", 6 => "v6", 7 => "v7", 8 => "v8", _ => "v9",
            }));
        }
        for k in 0..10 {
            p.push(I::Drop(match k {
                0 => "v0", 1 => "v1", 2 => "v2", 3 => "v3", 4 => "v4",
                5 => "v5", 6 => "v6", 7 => "v7", 8 => "v8", _ => "v9",
            }));
        }
        p.push(I::Push(6.0));
        p.push(I::Halt);
        p
    }));
    v.push(("B11_drop_then_arithmetic", vec![
        I::Push(5.0), I::Store("i"), I::Push(6.0), I::Push(7.0), I::Add,
        I::Drop("i"), I::Halt,
    ]));
    v.push(("B12_drop_below_live_compare", vec![
        I::Push(2.0), I::Store("i"), I::Push(3.0), I::Push(5.0), I::Lt,
        I::Drop("i"), I::Halt,
    ]));
    v.push(("B13_drops_interleaved_depths", vec![
        I::Push(1.0), I::Store("a"), I::Push(2.0), I::Push(3.0),
        I::Store("b"), I::Drop("a"), I::Push(4.0), I::Drop("b"), I::Halt,
    ]));
    v.push(("B14_drop_leaves_depth2", vec![
        I::Push(1.0), I::Push(2.0), I::Push(3.0), I::Store("a"),
        I::Drop("a"), I::Halt,
    ]));
    v.push(("B15_two_bind_drop_blocks", vec![
        I::Push(1.0), I::Store("a"), I::Drop("a"), I::Push(2.0),
        I::Store("b"), I::Drop("b"), I::Push(3.0), I::Push(4.0), I::Halt,
    ]));
    v.push(("B16_drop_after_assign", vec![
        I::Push(1.0), I::Store("i"), I::Push(6.0), I::Assign("i"),
        I::Push(1.0), I::Drop("i"), I::Halt,
    ]));
    // Context (14): Drop around loops, branches and computed values.
    v.push(("B17_drop_before_loop", vec![
        I::Push(5.0), I::Store("i"), I::Drop("i"), I::Push(0.0),
        I::Store("k"), I::L(0), I::Load("k"), I::Push(2.0), I::Lt,
        I::JumpIfFalse(1), I::Load("k"), I::Push(1.0), I::Add,
        I::Assign("k"), I::Jump(0), I::L(1), I::Load("k"), I::Halt,
    ]));
    v.push(("B18_drop_after_loop", vec![
        I::Push(0.0), I::Store("i"), I::L(0), I::Load("i"), I::Push(3.0),
        I::Lt, I::JumpIfFalse(1), I::Load("i"), I::Push(1.0), I::Add,
        I::Assign("i"), I::Jump(0), I::L(1), I::Push(8.0), I::Drop("i"),
        I::Halt,
    ]));
    v.push(("B19_drop_in_taken_branch", vec![
        I::Push(1.0), I::Push(2.0), I::Lt, I::Store("c"), I::Load("c"),
        I::JumpIfFalse(1), I::Push(6.0), I::Store("t"), I::Drop("t"),
        I::L(1), I::Push(9.0), I::Halt,
    ]));
    v.push(("B20_drop_in_skipped_branch", vec![
        I::Push(1.0), I::Push(2.0), I::Gt, I::Store("c"), I::Load("c"),
        I::JumpIfFalse(1), I::Push(6.0), I::Store("t"), I::Drop("t"),
        I::L(1), I::Push(9.0), I::Halt,
    ]));
    v.push(("B21_drop_with_loop_result_below", vec![
        I::Push(0.0), I::Store("i"), I::L(0), I::Load("i"), I::Push(3.0),
        I::Lt, I::JumpIfFalse(1), I::Load("i"), I::Push(1.0), I::Add,
        I::Assign("i"), I::Jump(0), I::L(1), I::Push(7.0), I::Push(8.0),
        I::Drop("i"), I::Halt,
    ]));
    v.push(("B22_drops_in_two_branches", vec![
        I::Push(1.0), I::Store("a"), I::Push(1.0), I::Store("b"),
        I::PushBool(true), I::JumpIfFalse(1), I::Push(1.0), I::Drop("a"),
        I::L(1), I::PushBool(true), I::JumpIfFalse(2), I::Push(1.0),
        I::Drop("b"), I::L(2), I::Push(9.0), I::Halt,
    ]));
    v.push(("B23_drops_between_two_loops", vec![
        I::Push(0.0), I::Store("i"), I::L(0), I::Load("i"), I::Push(2.0),
        I::Lt, I::JumpIfFalse(1), I::Load("i"), I::Push(1.0), I::Add,
        I::Assign("i"), I::Jump(0), I::L(1), I::Drop("i"), I::Push(0.0),
        I::Store("j"), I::L(2), I::Load("j"), I::Push(2.0), I::Lt,
        I::JumpIfFalse(3), I::Load("j"), I::Push(1.0), I::Add,
        I::Assign("j"), I::Jump(2), I::L(3), I::Drop("j"), I::Push(5.0),
        I::Halt,
    ]));
    v.push(("B24_dropped_slot_loop_uses_kept_slot", vec![
        I::Push(0.0), I::Store("b"), I::Drop("b"), I::Push(3.0),
        I::Store("a"), I::Push(0.0), I::Store("k"), I::L(0),
        I::Load("k"), I::Load("a"), I::Lt, I::JumpIfFalse(1),
        I::Load("k"), I::Push(1.0), I::Add, I::Assign("k"), I::Jump(0),
        I::L(1), I::Load("k"), I::Halt,
    ]));
    v.push(("B25_drop_with_computed_binding", vec![
        I::Push(2.0), I::Push(3.0), I::Mul, I::Store("p"), I::Push(1.0),
        I::Drop("p"), I::Push(6.0), I::Push(7.0), I::Add, I::Halt,
    ]));
    v.push(("B26_drop_pair_then_compare_fresh", vec![
        I::Push(1.0), I::Store("x"), I::Push(2.0), I::Store("y"),
        I::Drop("x"), I::Drop("y"), I::Push(4.0), I::Push(9.0), I::Lt,
        I::Halt,
    ]));
    v.push(("B27_drop_in_nested_branch", vec![
        I::PushBool(true), I::JumpIfFalse(3), I::PushBool(true),
        I::JumpIfFalse(2), I::Push(1.0), I::Store("t"), I::Drop("t"),
        I::L(2), I::L(3), I::Push(3.0), I::Halt,
    ]));
    v.push(("B28_two_bool_slots_one_dropped", vec![
        I::Push(3.0), I::Push(9.0), I::Lt, I::Store("r"), I::Push(2.0),
        I::Push(1.0), I::Lt, I::Store("s"), I::Push(5.0), I::Drop("r"),
        I::Push(6.0), I::Halt,
    ]));
    v.push(("B29_drops_on_deep_stack", vec![
        I::Push(1.0), I::Push(2.0), I::Push(3.0), I::Push(4.0),
        I::Store("a"), I::Store("b"), I::Push(5.0), I::Drop("a"),
        I::Drop("b"), I::Halt,
    ]));
    v.push(("B30_compare_drop_slot_use_after_loop", vec![
        I::Push(0.0), I::Store("i"), I::L(0), I::Load("i"), I::Push(3.0),
        I::Lt, I::JumpIfFalse(1), I::Load("i"), I::Push(1.0), I::Add,
        I::Assign("i"), I::Jump(0), I::L(1), I::Load("i"), I::Push(3.0),
        I::Eq, I::Store("c"), I::Push(9.0), I::Drop("c"), I::Load("i"),
        I::Halt,
    ]));
    // Negative (20): correct programs with adjacent shapes; nothing may
    // change. (Drop with an empty value stack at Halt is outside the
    // differential contract: the interpreter yields no value there and the
    // tiers yield 0; documented as CEP-ENGINE-014, not exercised here.)
    v.push(("B31_no_drop_baseline", vec![
        I::Push(7.0), I::Store("i"), I::Load("i"), I::Halt,
    ]));
    v.push(("B32_load_then_drop", vec![
        I::Push(2.0), I::Store("i"), I::Load("i"), I::Drop("i"), I::Halt,
    ]));
    v.push(("B33_depth2_no_drop", vec![
        I::Push(1.0), I::Push(2.0), I::Halt,
    ]));
    v.push(("B34_two_slots_no_drop", vec![
        I::Push(1.0), I::Push(2.0), I::Store("a"), I::Halt,
    ]));
    v.push(("B35_drop_top_slot_keep_bottom", vec![
        I::Push(1.0), I::Push(2.0), I::Store("a"), I::Drop("a"), I::Halt,
    ]));
    v.push(("B36_compare_store_drop_value", vec![
        I::Push(2.0), I::Push(5.0), I::Lt, I::Store("r"), I::Push(3.0),
        I::Drop("r"), I::Halt,
    ]));
    v.push(("B37_assign_then_drop", vec![
        I::Push(1.0), I::Store("i"), I::Push(6.0), I::Assign("i"),
        I::Push(4.0), I::Drop("i"), I::Halt,
    ]));
    v.push(("B38_drop_at_depth3", vec![
        I::Push(1.0), I::Push(2.0), I::Push(3.0), I::Store("a"),
        I::Drop("a"), I::Halt,
    ]));
    v.push(("B39_drop_keeps_depth3", vec![
        I::Push(1.0), I::Push(2.0), I::Push(3.0), I::Store("a"),
        I::Push(9.0), I::Drop("a"), I::Halt,
    ]));
    v.push(("B40_bool_store_drop_bool_result", vec![
        I::PushBool(true), I::Store("b"), I::Push(8.0), I::Drop("b"),
        I::PushBool(false), I::Halt,
    ]));
    v.push(("B41_drop_ladder", vec![
        I::Push(1.0), I::Store("a"), I::Push(2.0), I::Store("b"),
        I::Drop("a"), I::Push(3.0), I::Store("c"), I::Drop("b"),
        I::Push(4.0), I::Drop("c"), I::Push(5.0), I::Halt,
    ]));
    v.push(("B42_drop_then_jump_over_dead_code", vec![
        I::Push(1.0), I::Push(2.0), I::Lt, I::Store("c"), I::Load("c"),
        I::JumpIfFalse(1), I::Push(7.0), I::Store("t"), I::Drop("t"),
        I::Jump(2), I::L(1), I::Push(0.0), I::L(2), I::Push(9.0), I::Halt,
    ]));
    v.push(("B43_drop_and_straight_compares", vec![
        I::Push(1.0), I::Push(2.0), I::Lt, I::Store("c"), I::Drop("c"),
        I::Push(5.0), I::Push(9.0), I::Gt, I::Halt,
    ]));
    v.push(("B44_five_slots_one_drop", vec![
        I::Push(1.0), I::Push(2.0), I::Push(3.0), I::Push(4.0),
        I::Push(5.0), I::Store("v0"), I::Store("v1"), I::Store("v2"),
        I::Store("v3"), I::Store("v4"), I::Drop("v2"), I::Push(6.0),
        I::Halt,
    ]));
    v.push(("B45_drop_slot_bound_from_eq", vec![
        I::Push(3.0), I::Push(3.0), I::Eq, I::Store("e"), I::Push(2.0),
        I::Drop("e"), I::Halt,
    ]));
    v.push(("B46_two_drops_descending_depths", vec![
        I::Push(1.0), I::Push(2.0), I::Push(3.0), I::Store("x"),
        I::Store("y"), I::Drop("x"), I::Drop("y"), I::Halt,
    ]));
    v.push(("B47_drop_then_subtract", vec![
        I::Push(9.0), I::Store("m"), I::Push(5.0), I::Push(3.0), I::Sub,
        I::Drop("m"), I::Halt,
    ]));
    v.push(("B48_drop_before_eq", vec![
        I::Push(4.0), I::Store("n"), I::Drop("n"), I::Push(1.0),
        I::Push(1.0), I::Eq, I::Halt,
    ]));
    v.push(("B49_drop_ladder_deep_bottom", vec![
        I::Push(1.0), I::Push(2.0), I::Push(3.0), I::Push(4.0),
        I::Push(5.0), I::Store("s"), I::Drop("s"), I::Halt,
    ]));
    v.push(("B50_loop_drop_deep_return", vec![
        I::Push(0.0), I::Store("i"), I::L(0), I::Load("i"), I::Push(4.0),
        I::Lt, I::JumpIfFalse(1), I::Load("i"), I::Push(1.0), I::Add,
        I::Assign("i"), I::Jump(0), I::L(1), I::Push(1.0), I::Push(2.0),
        I::Drop("i"), I::Halt,
    ]));
    v
}

/// The matrices are exactly fifty cases each (standard 61.3 quota).
#[test]
fn matrices_are_fifty_cases_each() {
    assert_eq!(matrix_a().len(), 50, "matrix A quota");
    assert_eq!(matrix_b().len(), 50, "matrix B quota");
    assert_eq!(matrix_c().len(), 50, "matrix C quota");
}

/// Matrix A: every case must match the interpreter on all tiers.
#[test]
fn matrix_a_comparison_stack_discipline() {
    for (name, code) in matrix_a() {
        let bytes = asm(&code);
        assert_parity(name, &bytes);
    }
}

/// Matrix B: every case must match the interpreter on all tiers.
#[test]
fn matrix_b_drop_depth_simulation() {
    for (name, code) in matrix_b() {
        let bytes = asm(&code);
        assert_parity(name, &bytes);
    }
}

/// Matrix C: IR lift environment, phi and deopt-state class (50 cases).
///
/// CEP:TEST-CLASS: Regression Matrix
/// CEP:TEST-TARGET: ir::lift per-block environment snapshots; loop-header
///                  phis and header-safepoint deopt state; branch
///                  live-value refusal.
/// CEP:TEST-QUOTA: 50 tests (1 trigger, 5 boundary, 10 type/size, 14
///                 context, 20 negative).
/// CEP:TEST-EVIDENCE: pre-fix, C01 returned uninitialized machine memory on
///                  T1/T2/T3 (trip-0 loops), the diamond-in-loop shape
///                  mis-patched the loop phi and spun forever, and branch
///                  shapes carrying live values were silently dropped.
fn matrix_c() -> Vec<(&'static str, Vec<I>)> {
    let mut v: Vec<(&'static str, Vec<I>)> = Vec::new();
    // Trigger (1): trip-0 loop; pre-fix the exit path read the back-edge
    // value's slot (uninitialized) on every IR tier.
    v.push(("C01_trigger_trip0_loop", while_lt(0.0, 0.0, INC)));
    // Boundary (5).
    v.push(("C02_trip0_with_post_compare", {
        // while i < 0 { i += 1 }; if i == 0 { i = 7 } -> 7
        let mut p = while_lt(0.0, 0.0, INC);
        p.pop();
        p.pop();
        p.extend([
            I::Load("i"),
            I::Push(0.0),
            I::Eq,
            I::JumpIfFalse(4),
            I::Push(7.0),
            I::Assign("i"),
            I::L(4),
            I::Load("i"),
            I::Halt,
        ]);
        p
    }));
    v.push(("C03_trip1_loop", while_lt(0.0, 1.0, INC)));
    v.push(("C04_trip2_loop", while_lt(0.0, 2.0, INC)));
    v.push(("C05_loop_never_runs_return_const", {
        // while i < 0 { i += 1 }; return 5
        let mut p = while_lt(0.0, 0.0, INC);
        p.pop();
        p.pop();
        p.push(I::Push(5.0));
        p.push(I::Halt);
        p
    }));
    v.push(("C06_two_loops_first_trip0", {
        let mut p = while_lt(0.0, 0.0, INC);
        p.pop();
        p.pop();
        p.push(I::Push(0.0));
        p.push(I::Store("j"));
        p.extend([
            I::L(5),
            I::Load("j"),
            I::Push(3.0),
            I::Lt,
            I::JumpIfFalse(6),
            I::Load("j"),
            I::Push(1.0),
            I::Add,
            I::Assign("j"),
            I::Jump(5),
            I::L(6),
            I::Load("j"),
            I::Halt,
        ]);
        p
    }));
    // Type/size (10).
    v.push(("C07_bool_carried_trip0", {
        // r = false; i = 0; while i < 0 { r = true; i += 1 }; return r
        vec![
            I::PushBool(false),
            I::Store("r"),
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(0.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::PushBool(true),
            I::Assign("r"),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("r"),
            I::Halt,
        ]
    }));
    v.push(("C08_bool_carried_trip3", {
        // r = false; i = 0; while i < 3 { r = (i < 1); i += 1 } -> true
        vec![
            I::PushBool(false),
            I::Store("r"),
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(3.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(1.0),
            I::Lt,
            I::Assign("r"),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("r"),
            I::Halt,
        ]
    }));
    v.push(("C09_two_carried_counters", {
        // i = 0; j = 5; while i < 3 { i += 1; j += 2 } -> 11
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::Push(5.0),
            I::Store("j"),
            I::L(0),
            I::Load("i"),
            I::Push(3.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Load("j"),
            I::Push(2.0),
            I::Add,
            I::Assign("j"),
            I::Jump(0),
            I::L(1),
            I::Load("j"),
            I::Halt,
        ]
    }));
    v.push((
        "C10_countdown_carried",
        while_gt(5.0, 0.0, &[I::Load("i"), I::Push(1.0), I::Sub, I::Assign("i")]),
    ));
    v.push(("C11_diamond_then_arm_assigns", {
        // i = 0; while i < 5 { if i > 1 { i += 2 } else { i += 1 } } -> 6
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(5.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(1.0),
            I::Gt,
            I::JumpIfFalse(2),
            I::Load("i"),
            I::Push(2.0),
            I::Add,
            I::Assign("i"),
            I::Jump(3),
            I::L(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::L(3),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("C12_diamond_else_arm_assigns", {
        // i = 0; while i < 4 { if i > 9 { i += 2 } else { i += 1 } } -> 4
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(4.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(9.0),
            I::Gt,
            I::JumpIfFalse(2),
            I::Load("i"),
            I::Push(2.0),
            I::Add,
            I::Assign("i"),
            I::Jump(3),
            I::L(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::L(3),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("C13_nested_diamond_in_loop", {
        // i = 0; while i < 3 { if i < 1 { i += 1 } else { if i < 2
        // { i += 2 } else { i += 3 } } } -> 6
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(3.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(1.0),
            I::Lt,
            I::JumpIfFalse(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(4),
            I::L(2),
            I::Load("i"),
            I::Push(2.0),
            I::Lt,
            I::JumpIfFalse(3),
            I::Load("i"),
            I::Push(2.0),
            I::Add,
            I::Assign("i"),
            I::Jump(4),
            I::L(3),
            I::Load("i"),
            I::Push(3.0),
            I::Add,
            I::Assign("i"),
            I::L(4),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("C14_carried_in_both_arms", {
        // i = 0; while i < 6 { if i > 2 { i += 1 } else { i += 1 } } -> 6
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(6.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(2.0),
            I::Gt,
            I::JumpIfFalse(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(3),
            I::L(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::L(3),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("C15_loop_bound_by_prior_result", {
        // while i < 2 { i += 1 }; while j < i { j += 1 } -> 2
        let mut p = while_lt(0.0, 2.0, INC);
        p.pop();
        p.pop();
        p.extend([
            I::Push(0.0),
            I::Store("j"),
            I::L(5),
            I::Load("j"),
            I::Load("i"),
            I::Lt,
            I::JumpIfFalse(6),
            I::Load("j"),
            I::Push(1.0),
            I::Add,
            I::Assign("j"),
            I::Jump(5),
            I::L(6),
            I::Load("j"),
            I::Halt,
        ]);
        p
    }));
    v.push(("C16_large_constant_carried", while_lt(0.0, 4.0, &[I::Load("i"), I::Push(1000.0), I::Add, I::Assign("i")])));
    // Context (14).
    v.push(("C17_post_loop_if_assign", {
        let mut p = while_lt(0.0, 2.0, INC);
        p.pop();
        p.pop();
        p.extend([
            I::Load("i"),
            I::Push(2.0),
            I::Eq,
            I::JumpIfFalse(4),
            I::Push(9.0),
            I::Assign("i"),
            I::L(4),
            I::Load("i"),
            I::Halt,
        ]);
        p
    }));
    v.push(("C18_if_before_loop", {
        // i = 0; if 1 < 2 { i = 4 }; while i < 6 { i += 1 } -> 6. The if is
        // a diamond with one empty arm; the join carries a phi in T1+.
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::Push(1.0),
            I::Push(2.0),
            I::Lt,
            I::JumpIfFalse(9),
            I::Push(4.0),
            I::Assign("i"),
            I::L(9),
            I::L(7),
            I::Load("i"),
            I::Push(6.0),
            I::Lt,
            I::JumpIfFalse(8),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(7),
            I::L(8),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("C19_nested_inner_trip0", {
        // j = 0; while j < 2 { i = 0; while i < 0 { i += 1 }; j += 1 } -> 2
        vec![
            I::Push(0.0),
            I::Store("j"),
            I::L(0),
            I::Load("j"),
            I::Push(2.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Push(0.0),
            I::Store("i"),
            I::L(2),
            I::Load("i"),
            I::Push(0.0),
            I::Lt,
            I::JumpIfFalse(3),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(2),
            I::L(3),
            I::Load("j"),
            I::Push(1.0),
            I::Add,
            I::Assign("j"),
            I::Jump(0),
            I::L(1),
            I::Load("j"),
            I::Halt,
        ]
    }));
    v.push(("C20_nested_outer_trip0", {
        // j = 0; while j < 0 { i = 0; while i < 3 { i += 1 }; j += 1 } -> 0
        vec![
            I::Push(0.0),
            I::Store("j"),
            I::L(0),
            I::Load("j"),
            I::Push(0.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Push(0.0),
            I::Store("i"),
            I::L(2),
            I::Load("i"),
            I::Push(3.0),
            I::Lt,
            I::JumpIfFalse(3),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(2),
            I::L(3),
            I::Load("j"),
            I::Push(1.0),
            I::Add,
            I::Assign("j"),
            I::Jump(0),
            I::L(1),
            I::Load("j"),
            I::Halt,
        ]
    }));
    v.push(("C21_eq_head_jumpiftrue", {
        // i = 5; while !(i == 0) { i -= 1 } via Eq head + JumpIfTrue -> 0
        vec![
            I::Push(5.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(0.0),
            I::Eq,
            I::JumpIfTrue(1),
            I::Load("i"),
            I::Push(1.0),
            I::Sub,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("C22_two_sequential_diamonds", {
        // c = (1<2 ? 4 : 0); d = (c < 1 ? 7 : 8) -> 8
        let mut p = diamond(1.0, I::Lt, 2.0, 4.0, 0.0, &[]);
        p.extend(vec![
            I::Load("c"),
            I::Push(1.0),
            I::Lt,
            I::JumpIfFalse(5),
            I::Push(7.0),
            I::Store("d"),
            I::Jump(6),
            I::L(5),
            I::Push(8.0),
            I::Store("d"),
            I::L(6),
            I::Load("d"),
            I::Halt,
        ]);
        p
    }));
    v.push(("C23_loop_then_drop_use_other", {
        // while i < 2 { i += 1 }; u = 8; drop u; return i
        let mut p = while_lt(0.0, 2.0, INC);
        p.pop();
        p.pop();
        p.extend([
            I::Push(8.0),
            I::Store("u"),
            I::Drop("u"),
            I::Load("i"),
            I::Halt,
        ]);
        p
    }));
    v.push(("C24_arms_assign_different_slots", {
        // i = 0; a = 0; b = 0; while i < 3 { if i < 1 { a = i + 1 } else
        // { b = i + 1 }; i += 1 } -> 3
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::Push(0.0),
            I::Store("a"),
            I::Push(0.0),
            I::Store("b"),
            I::L(0),
            I::Load("i"),
            I::Push(3.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(1.0),
            I::Lt,
            I::JumpIfFalse(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Store("a"),
            I::Jump(3),
            I::L(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Store("b"),
            I::L(3),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("b"),
            I::Halt,
        ]
    }));
    v.push(("C25_condition_two_carried", {
        // i = 0; j = 0; while i < j + 3 { i += 1 } -> 3. Only i increments
        // (incrementing both would make the condition always true: an
        // infinite program, out of scope for a termination-testing suite).
        // The head computes the bound from the carried j.
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::Push(0.0),
            I::Store("j"),
            I::L(0),
            I::Load("i"),
            I::Load("j"),
            I::Push(3.0),
            I::Add,
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("C26_eq_head_countdown_by_two", {
        // i = 6; while !(i == 0) { i -= 2 } -> 0
        vec![
            I::Push(6.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(0.0),
            I::Eq,
            I::JumpIfTrue(1),
            I::Load("i"),
            I::Push(2.0),
            I::Sub,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("C27_mul_carried", while_lt(1.0, 64.0, &[I::Load("i"), I::Push(2.0), I::Mul, I::Assign("i")])));
    v.push(("C28_diamond_on_second_counter", {
        // i = 0; j = 0; while i < 4 { if j < 1 { j += 1 }; i += 1 } -> 1
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::Push(0.0),
            I::Store("j"),
            I::L(0),
            I::Load("i"),
            I::Push(4.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("j"),
            I::Push(1.0),
            I::Lt,
            I::JumpIfFalse(2),
            I::Load("j"),
            I::Push(1.0),
            I::Add,
            I::Assign("j"),
            I::L(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("j"),
            I::Halt,
        ]
    }));
    v.push(("C29_joint_break_exit", {
        // i = 0; j = 0; while i < 10 { j += 2; if j == 6 break; i += 1 }
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::Push(0.0),
            I::Store("j"),
            I::L(0),
            I::Load("i"),
            I::Push(10.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("j"),
            I::Push(2.0),
            I::Add,
            I::Assign("j"),
            I::Load("j"),
            I::Push(6.0),
            I::Eq,
            I::JumpIfTrue(1),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v.push(("C30_trip0_then_compares", {
        // while i < 0 { i += 1 }; return 2<3 with a second compare stacked
        let mut p = while_lt(0.0, 0.0, INC);
        p.pop();
        p.pop();
        p.extend([
            I::Push(2.0),
            I::Push(3.0),
            I::Lt,
            I::Push(5.0),
            I::Push(5.0),
            I::Eq,
            I::Halt,
        ]);
        p
    }));
    // Negative (20): balanced branch shapes; every tier stays identical.
    v.push(("C31_diamond_lt_true", diamond(1.0, I::Lt, 2.0, 4.0, 0.0, &[I::Load("c"), I::Halt])));
    v.push(("C32_diamond_lt_false", diamond(2.0, I::Lt, 1.0, 4.0, 0.0, &[I::Load("c"), I::Halt])));
    v.push(("C33_diamond_gt_true", diamond(5.0, I::Gt, 1.0, 4.0, 0.0, &[I::Load("c"), I::Halt])));
    v.push(("C34_diamond_gt_false", diamond(1.0, I::Gt, 5.0, 4.0, 0.0, &[I::Load("c"), I::Halt])));
    v.push(("C35_diamond_eq_true", diamond(3.0, I::Eq, 3.0, 4.0, 0.0, &[I::Load("c"), I::Halt])));
    v.push(("C36_diamond_eq_false", diamond(3.0, I::Eq, 4.0, 4.0, 0.0, &[I::Load("c"), I::Halt])));
    v.push(("C37_diamond_fractions", diamond(0.25, I::Lt, 0.5, 1.0, 2.0, &[I::Load("c"), I::Halt])));
    v.push(("C38_diamond_negatives", diamond(-3.0, I::Lt, -1.0, 1.0, 2.0, &[I::Load("c"), I::Halt])));
    v.push(("C39_diamond_computed_arms", {
        // if 2 < 3 { c = 2+3 } else { c = 2*3 } -> 5
        let mut p = vec![I::Push(2.0), I::Push(3.0), I::Lt, I::JumpIfFalse(1)];
        p.extend([I::Push(2.0), I::Push(3.0), I::Add, I::Store("c"), I::Jump(2)]);
        p.extend([I::L(1), I::Push(2.0), I::Push(3.0), I::Mul, I::Store("c"), I::L(2)]);
        p.extend([I::Load("c"), I::Halt]);
        p
    }));
    v.push(("C40_diamond_feeds_diamond", {
        // if 1 < 2 { c = 1 } else { c = 0 }; if c < 1 { d = 7 } else
        // { d = 8 } -> 7
        let mut p = diamond(1.0, I::Lt, 2.0, 1.0, 0.0, &[]);
        p.extend(vec![
            I::Load("c"),
            I::Push(1.0),
            I::Lt,
            I::JumpIfFalse(3),
            I::Push(7.0),
            I::Store("d"),
            I::Jump(4),
            I::L(3),
            I::Push(8.0),
            I::Store("d"),
            I::L(4),
            I::Load("d"),
            I::Halt,
        ]);
        p
    }));
    v.push(("C41_branch_no_loop_bool_result", {
        // r = false; if 5 < 9 { r = true }; return r -> true
        vec![
            I::PushBool(false),
            I::Store("r"),
            I::Push(5.0),
            I::Push(9.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::PushBool(true),
            I::Assign("r"),
            I::L(1),
            I::Load("r"),
            I::Halt,
        ]
    }));
    v.push(("C42_compare_chain_into_branch", {
        // r = (1<2) ? (3<4) : (5<6) -> true
        vec![
            I::Push(1.0),
            I::Push(2.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Push(3.0),
            I::Push(4.0),
            I::Lt,
            I::Store("r"),
            I::Jump(2),
            I::L(1),
            I::Push(5.0),
            I::Push(6.0),
            I::Lt,
            I::Store("r"),
            I::L(2),
            I::Load("r"),
            I::Halt,
        ]
    }));
    v.push(("C43_depth_below_balanced_branch", {
        // r0 = (1<2) is live below the branch: the IR lift refuses the
        // shape (UnbalancedStack, branch stack must be empty) and the
        // engine falls back to the interpreter; the result stays correct
        // on every tier. This pins the refusal contract, not a value.
        vec![
            I::Push(1.0),
            I::Push(2.0),
            I::Lt,
            I::Push(1.0),
            I::Push(2.0),
            I::Gt,
            I::JumpIfFalse(1),
            I::Push(30.0),
            I::Jump(2),
            I::L(1),
            I::Push(40.0),
            I::L(2),
            I::Halt, // bottom = r0 = true
        ]
    }));
    v.push(("C44_three_way_chain", {
        // if 1>2 {c=1} elif 2>3 {c=2} else {c=3} -> 3
        vec![
            I::Push(1.0),
            I::Push(2.0),
            I::Gt,
            I::JumpIfFalse(1),
            I::Push(1.0),
            I::Store("c"),
            I::Jump(4),
            I::L(1),
            I::Push(2.0),
            I::Push(3.0),
            I::Gt,
            I::JumpIfFalse(2),
            I::Push(2.0),
            I::Store("c"),
            I::Jump(4),
            I::L(2),
            I::Push(3.0),
            I::Store("c"),
            I::L(4),
            I::Load("c"),
            I::Halt,
        ]
    }));
    v.push(("C45_diamond_equal_arm_values", diamond(1.0, I::Lt, 2.0, 5.0, 5.0, &[I::Load("c"), I::Halt])));
    v.push(("C46_branch_over_dead_code", {
        // if 1 < 2 { c = 1; jump end } else { c = 2 }; end: return c -> 1
        vec![
            I::Push(1.0),
            I::Push(2.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Push(1.0),
            I::Store("c"),
            I::Jump(2),
            I::L(1),
            I::Push(2.0),
            I::Store("c"),
            I::L(2),
            I::Load("c"),
            I::Halt,
        ]
    }));
    v.push(("C47_diamond_zero_operands", diamond(0.0, I::Lt, 0.0, 1.0, 2.0, &[I::Load("c"), I::Halt])));
    v.push(("C48_diamond_large_values", diamond(100000.0, I::Lt, 100001.0, 1.0, 2.0, &[I::Load("c"), I::Halt])));
    v.push(("C49_diamond_jumpiftrue_polarity", {
        vec![
            I::Push(1.0),
            I::Push(2.0),
            I::Lt,
            I::JumpIfTrue(1),
            I::Push(9.0),
            I::Store("c"),
            I::Jump(2),
            I::L(1),
            I::Push(8.0),
            I::Store("c"),
            I::L(2),
            I::Load("c"),
            I::Halt,
        ]
    }));
    v.push(("C50_loop_one_trip_with_diamond", {
        // while i < 1 { if i < 5 { i += 1 } else { i += 9 } } -> 1
        vec![
            I::Push(0.0),
            I::Store("i"),
            I::L(0),
            I::Load("i"),
            I::Push(1.0),
            I::Lt,
            I::JumpIfFalse(1),
            I::Load("i"),
            I::Push(5.0),
            I::Lt,
            I::JumpIfFalse(2),
            I::Load("i"),
            I::Push(1.0),
            I::Add,
            I::Assign("i"),
            I::Jump(3),
            I::L(2),
            I::Load("i"),
            I::Push(9.0),
            I::Add,
            I::Assign("i"),
            I::L(3),
            I::Jump(0),
            I::L(1),
            I::Load("i"),
            I::Halt,
        ]
    }));
    v
}

/// Matrix C: every case must match the interpreter on all tiers.
#[test]
fn matrix_c_ir_lift_environment() {
    for (name, code) in matrix_c() {
        let bytes = asm(&code);
        assert_parity(name, &bytes);
    }
}

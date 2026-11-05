// CEP:FILE: engine/tests/stack_discipline/matrix_a.rs
// CEP:WHAT: Matrix A (50 cases) for the confirmed compare-template defect:
//           LessThan/GreaterThan/Equal pushed their boolean result instead
//           of collapsing the operand pair in place, leaking 16
//           machine-stack bytes per execution and overflowing inside
//           loops.
// CEP:WHY: Standard 61.3: a fix is not understood until 50 variations are
//          proven safe. Distribution: 1 trigger + 5 boundary + 10
//          type/size + 14 context + 20 negative.
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: any tier diverging from the interpreter fails with the
//              case name, the tier and both values.
// CEP:ASSUMES: harness invariants (one value on the stack at Halt).
// CEP:COST: offline.
// CEP:EVIDENCE: the trigger case reproduces the pre-fix loop stack
//               overflow; negatives prove correct programs unchanged.
// CEP:TEST-CLASS: Regression Matrix
// CEP:TEST-TARGET: T0 compare/equal templates (in-place collapse).
// CEP:TEST-QUOTA: 50 tests (1 trigger, 5 boundary, 10 type/size, 14
//                 context, 20 negative).
// CEP:TEST-EVIDENCE: differential parity on Interp/T0/T1/T2/T3.
// CEP:SECURITY: test-authored bytecode only.

use crate::harness::*;

/// CEP:WHAT: Matrix A case list: 50 comparison-stack-discipline programs.
/// CEP:WHY: Standard 61.3: fifty variations prove the compare/equal in-place
/// collapse fix; distribution 1 trigger + 5 boundary + 10 type/size +
/// 14 context + 20 negative.
/// CEP:STATUS: complete
/// CEP:FAILURE: any tier diverging from the interpreter fails with
///              the case name, the tier and both values
/// CEP:ASSUMES: harness invariants
/// CEP:COST: offline
/// CEP:EVIDENCE: the trigger case reproduces the pre-fix loop stack overflow.
pub(crate) fn matrix_a() -> Vec<(&'static str, Vec<I>)> {
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

/// CEP:WHAT: Matrix A differential runner.
/// CEP:WHY: Each case assembles and runs on Interp/T0/T1/T2/T3; any divergence
/// fails the run.
/// CEP:STATUS: complete
/// CEP:FAILURE: any tier diverging from the interpreter fails with
///              the case name, the tier and both values
/// CEP:ASSUMES: harness invariants
/// CEP:COST: offline
/// CEP:EVIDENCE: assert_parity covers all five tiers per case.
/// Matrix A: every case must match the interpreter on all tiers.
#[test]
fn matrix_a_comparison_stack_discipline() {
    for (name, code) in matrix_a() {
        let bytes = asm(&code);
        assert_parity(name, &bytes);
    }
}

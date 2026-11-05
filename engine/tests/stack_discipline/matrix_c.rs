// CEP:FILE: engine/tests/stack_discipline/matrix_c.rs
// CEP:WHAT: Matrix C (50 cases) for the confirmed IR-lift environment
//           defect class: per-block environment snapshots at branch time,
//           loop-header phi construction and header-safepoint deopt
//           state. Pre-fix, trip-0 loops returned uninitialized machine
//           memory on T1/T2/T3, diamonds mis-patched loop phis, and
//           branches carrying live values were silently dropped.
// CEP:WHY: Standard 61.3: fifty variations per confirmed defect class;
//          each shape exercises one environment-transfer edge case.
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: any tier diverging from the interpreter fails with the
//              case name, the tier and both values.
// CEP:ASSUMES: harness invariants.
// CEP:COST: offline.
// CEP:EVIDENCE: the trigger case reproduces the pre-fix uninitialized
//               read; negatives prove correct programs unchanged.
// CEP:TEST-CLASS: Regression Matrix
// CEP:TEST-TARGET: ir::lift per-block environment snapshots; loop-header
//                  phis and header-safepoint deopt state; branch
//                  live-value refusal.
// CEP:TEST-QUOTA: 50 tests (1 trigger, 5 boundary, 10 type/size, 14
//                 context, 20 negative).
// CEP:TEST-EVIDENCE: differential parity on Interp/T0/T1/T2/T3.
// CEP:SECURITY: test-authored bytecode only.

use crate::harness::*;

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
/// CEP:WHAT: Matrix C case list: 50 IR-lift environment programs.
/// CEP:WHY: Standard 61.3: fifty variations prove the per-block environment
/// snapshot fix; trip-0 loops, diamonds, branch-live-value shapes.
/// CEP:STATUS: complete
/// CEP:FAILURE: any tier diverging from the interpreter fails with
///              the case name, the tier and both values
/// CEP:ASSUMES: harness invariants
/// CEP:COST: offline
/// CEP:EVIDENCE: the trigger case reproduces the pre-fix uninitialized read.
pub(crate) fn matrix_c() -> Vec<(&'static str, Vec<I>)> {
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

/// CEP:WHAT: Matrix C differential runner.
/// CEP:WHY: Each case assembles and runs on Interp/T0/T1/T2/T3; any divergence
/// fails the run.
/// CEP:STATUS: complete
/// CEP:FAILURE: any tier diverging from the interpreter fails with
///              the case name, the tier and both values
/// CEP:ASSUMES: harness invariants
/// CEP:COST: offline
/// CEP:EVIDENCE: assert_parity covers all five tiers per case.
/// Matrix C: every case must match the interpreter on all tiers.
#[test]
fn matrix_c_ir_lift_environment() {
    for (name, code) in matrix_c() {
        let bytes = asm(&code);
        assert_parity(name, &bytes);
    }
}

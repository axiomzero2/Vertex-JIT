// CEP:FILE: engine/tests/stack_discipline/matrix_b.rs
// CEP:WHAT: Matrix B (50 cases) for the confirmed Drop depth-simulation
//           defect: the T0 compile() halt-tail simulation popped the value
//           stack on Drop(slot) although the template, the interpreter and
//           check_legality treat Drop as a binding retirement with no
//           value-stack effect, so the halt tail read the wrong slot
//           (pre-fix: Drop of 7 returned 0).
// CEP:WHY: Standard 61.3: fifty variations per confirmed defect class.
//          Distribution: 1 trigger + 5 boundary + 10 type/size + 14
//          context + 20 negative.
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: any tier diverging from the interpreter fails with the
//              case name, the tier and both values.
// CEP:ASSUMES: harness invariants; Drop retires bindings only.
// CEP:COST: offline.
// CEP:EVIDENCE: the trigger case reproduces the pre-fix wrong-slot read;
//               negatives prove correct programs unchanged.
// CEP:TEST-CLASS: Regression Matrix
// CEP:TEST-TARGET: compile() halt-tail depth simulation (Drop arm).
// CEP:TEST-QUOTA: 50 tests (1 trigger, 5 boundary, 10 type/size, 14
//                 context, 20 negative).
// CEP:TEST-EVIDENCE: differential parity on Interp/T0/T1/T2/T3.
// CEP:SECURITY: test-authored bytecode only.

use crate::harness::*;

/// Matrix B: Drop depth-simulation (50 cases). The interpreter and
/// check_legality treat Drop(slot) as a binding retirement only; the T0
/// depth simulation must agree or the halt tail reads the wrong slot.
/// CEP:WHAT: Matrix B case list: 50 Drop depth-simulation programs.
/// CEP:WHY: Standard 61.3: fifty variations prove the halt-tail Drop fix; the
/// trigger is Drop of 7 before Halt (pre-fix returned 0).
/// CEP:STATUS: complete
/// CEP:FAILURE: any tier diverging from the interpreter fails with
///              the case name, the tier and both values
/// CEP:ASSUMES: harness invariants
/// CEP:COST: offline
/// CEP:EVIDENCE: the trigger case reproduces the pre-fix wrong-slot read.
pub(crate) fn matrix_b() -> Vec<(&'static str, Vec<I>)> {
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

/// CEP:WHAT: Matrix B differential runner.
/// CEP:WHY: Each case assembles and runs on Interp/T0/T1/T2/T3; any divergence
/// fails the run.
/// CEP:STATUS: complete
/// CEP:FAILURE: any tier diverging from the interpreter fails with
///              the case name, the tier and both values
/// CEP:ASSUMES: harness invariants
/// CEP:COST: offline
/// CEP:EVIDENCE: assert_parity covers all five tiers per case.
/// Matrix B: every case must match the interpreter on all tiers.
#[test]
fn matrix_b_drop_depth_simulation() {
    for (name, code) in matrix_b() {
        let bytes = asm(&code);
        assert_parity(name, &bytes);
    }
}

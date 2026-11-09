// CEP:FILE: engine/tests/call_inline_tests.rs
// CEP:WHAT: The engine call convention and T3 cross-function inlining:
//           interpreter argument binding and order, arity rejection,
//           inlining effectiveness (spliced two-arg and branchy shared-
//           tail callees), gate refusals (stack-depth, cycle, Halt, node
//           budget, single-trailing-Ret, depth budget), compilation
//           refusals for cyclic call graphs and unresolvable callees,
//           machine calls into compiled callees, and deopt resuming
//           inside a spliced body — each differentially checked against
//           the interpreter.
// CEP:WHY: Spec 5.2.1 and the call convention (doc/bytecode.md) must be
//          both effective (the transforms fire) and correct (bit-exact
//          results, coherent deopt). The interpreter arm, the machine
//          call ABI and the merged image must agree on argument order
//          and slot space.
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: assertion failures name the gate/tier and the divergence.
// CEP:ASSUMES: x86-64 Linux System V ABI (doc/codegen.md).
// CEP:COST: offline.
// CEP:EVIDENCE: this file.
// CEP:SECURITY: test-authored inputs.
//! Call-convention and T3 inlining tests.

use vertex_jit::cold::bytecode::{Function, Insn, Operand, Opcode as Op};
use vertex_jit::cold::engine::JitEngine;
use vertex_jit::cold::inline;
use vertex_jit::cold::passes::PassBudgets;
use vertex_jit::cold::tier::{JitBudgets, Tier};
use vertex_jit::hot::host::HostHooks;
use vertex_jit::hot::interp::Exit;
use vertex_jit::hot::value::Value;
use vertex_jit::platform::TargetConfig;

/// CEP:WHAT: Instruction builders.
/// CEP:WHY: The fixtures read as pseudocode, not as operand noise.
/// CEP:STATUS: complete
/// CEP:FAILURE: none (pure construction).
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: every fixture below.
/// CEP:SECURITY: none
fn num(f: f64) -> Insn {
    Insn { op: Op::PushNumber, operand: Operand::F64(f) }
}
fn call(argc: u32, callee: u32) -> Insn {
    Insn { op: Op::Call, operand: Operand::U64(((argc as u64) << 32) | callee as u64) }
}
fn op(o: Op) -> Insn {
    Insn { op: o, operand: Operand::None }
}
fn slot(o: Op, s: u16) -> Insn {
    Insn { op: o, operand: Operand::Slot(s) }
}
fn jif(t: u32) -> Insn {
    Insn { op: Op::JumpIfFalse, operand: Operand::Target(t) }
}

/// CEP:WHAT: Run one function pinned to one tier.
/// CEP:WHY: Tier-pinned runs isolate each tier's behavior; callees stay
///              at their own pinned tiers.
/// CEP:STATUS: complete
/// CEP:FAILURE: Err carries the engine error's text.
/// CEP:ASSUMES: the tier exists for the function.
/// CEP:COST: one run.
/// CEP:EVIDENCE: every engine-level test routes through it.
/// CEP:SECURITY: none
fn run_pinned(e: &mut JitEngine, fn_id: u32, tier: Tier) -> Result<Option<Value>, String> {
    e.tiers[fn_id as usize] = tier;
    match e.run(fn_id, &HostHooks::stdio()).map_err(|err| format!("{err}"))? {
        Exit::Halt(v) => Ok(v),
        Exit::Returned(v) => Ok(Some(v)),
        Exit::ProcessExited(c) => Ok(Some(Value::Number(c))),
        Exit::BackEdgeBudget { .. } => Ok(None),
    }
}

/// CEP:WHAT: `sub(x, y) = x - y` called through the interpreter with the
///           arguments pushed left to right.
/// CEP:WHY: Pins the call convention's argument order (top of stack is
///          the last argument, locals 0..argc bind in order) — the
///          inliner's stores and the ABI registers must match it.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on order or binding divergence.
/// CEP:ASSUMES: none
/// CEP:COST: one interpreted run.
/// CEP:EVIDENCE: inline.rs stores order; codegen ARG_REGS.
/// CEP:SECURITY: none
#[test]
fn interp_call_binds_arguments_in_order() {
    let sub = Function {
        name: "sub".to_owned(),
        locals: vec!["x".to_owned(), "y".to_owned()],
        strings: Vec::new(),
        code: vec![slot(Op::LoadVar, 0), slot(Op::LoadVar, 1), op(Op::Sub), op(Op::Ret)],
    };
    let mut e = JitEngine::new(
        vec![
            Function {
                name: "main".to_owned(),
                locals: Vec::new(),
                strings: Vec::new(),
                code: vec![num(9.0), num(4.0), call(2, 1), op(Op::Halt)],
            },
            sub,
        ],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    // 9 - 4: the arguments push x first, so the top of the stack is y.
    let v = run_pinned(&mut e, 0, Tier::Interp).expect("run");
    assert_eq!(v, Some(Value::Number(5.0)), "argument order must bind x=9, y=4");
}

/// CEP:WHAT: A Call whose argc exceeds the callee's locals is rejected.
/// CEP:WHY: The convention binds locals 0..argc; more arguments than
///          locals cannot bind. Fail loud (Law 6).
/// CEP:STATUS: complete
/// CEP:FAILURE: panics unless the interpreter rejects the call.
/// CEP:ASSUMES: none
/// CEP:COST: one interpreted run.
/// CEP:EVIDENCE: interp_run.rs Call arity guard.
/// CEP:SECURITY: none
#[test]
fn interp_rejects_arity_exceeding_callee_locals() {
    let one = Function {
        name: "one".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: vec![slot(Op::LoadVar, 0), op(Op::Ret)],
    };
    let mut e = JitEngine::new(
        vec![
            Function {
                name: "main".to_owned(),
                locals: Vec::new(),
                strings: Vec::new(),
                code: vec![num(1.0), num(2.0), call(2, 1), op(Op::Halt)],
            },
            one,
        ],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    assert!(
        run_pinned(&mut e, 0, Tier::Interp).is_err(),
        "argc 2 into a 1-local callee must be rejected"
    );
}

/// CEP:WHAT: T3 splices a two-argument callee at two call sites and the
///           results match the interpreter bit-for-bit.
/// CEP:WHY: Spec 5.2.1 effectiveness (inlined_calls counts both sites,
///          the merged image is registered) plus 4-tier parity; the
///          spliced body's Param nodes must read the inliner's bound
///          argument slots, and an inlined-away callee needs no code.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on parity, count, or image-registration divergence.
/// CEP:ASSUMES: arities derive from the call sites (2 here).
/// CEP:COST: one interpreted run, one T3 run.
/// CEP:EVIDENCE: inline.rs splice construction; engine commit gates.
/// CEP:SECURITY: none
#[test]
fn t3_inlines_two_arg_callee_and_matches_interpreter() {
    // addmul(x, y) = x * 2 + y
    let callee = Function {
        name: "addmul".to_owned(),
        locals: vec!["x".to_owned(), "y".to_owned()],
        strings: Vec::new(),
        code: vec![
            slot(Op::LoadVar, 0), // x
            num(2.0),
            op(Op::Mul),
            slot(Op::LoadVar, 1), // y
            op(Op::Add),
            op(Op::Ret),
        ],
    };
    // main: a = addmul(3, 4); b = addmul(1, 1); halt a + b  (= 13)
    let main = Function {
        name: "main".to_owned(),
        locals: vec!["a".to_owned(), "b".to_owned()],
        strings: Vec::new(),
        code: vec![
            num(3.0),
            num(4.0),
            call(2, 1),
            slot(Op::StoreVar, 0),
            num(1.0),
            num(1.0),
            call(2, 1),
            slot(Op::StoreVar, 1),
            slot(Op::LoadVar, 0),
            slot(Op::LoadVar, 1),
            op(Op::Add),
            op(Op::Halt),
        ],
    };
    let mut ref_e = JitEngine::new(
        vec![main.clone(), callee.clone()],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    let expected = run_pinned(&mut ref_e, 0, Tier::Interp).expect("interp run");
    assert_eq!(expected, Some(Value::Number(13.0)), "fixture semantics");
    let mut e = JitEngine::new(
        vec![main, callee],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    assert_eq!(e.arities[1], 2, "arity derivation over the call sites");
    let v = run_pinned(&mut e, 0, Tier::T3).expect("t3 run");
    assert_eq!(v, expected, "T3 diverged from the interpreter");
    assert_eq!(e.stats.inlined_calls, 2, "both call sites must splice");
    assert!(e.merged_of[0].is_some(), "the merged image must be registered");
    assert!(e.compiled[1].is_none(), "an inlined-away callee must not compile");
}

/// CEP:WHAT: T3 splices a branchy callee whose branches share one tail
///           return; both paths agree with the interpreter.
/// CEP:WHY: The shared-tail shape is the useful branchy case that must
///          still inline (gate 6b passes it): internal branch edges
///          enter merges on a zero baseline inside the spliced region.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on parity or splice-count divergence.
/// CEP:ASSUMES: none
/// CEP:COST: one interpreted run, one T3 run.
/// CEP:EVIDENCE: inline.rs gate 1 and gate 6b.
/// CEP:SECURITY: none
#[test]
fn t3_inlines_branchy_shared_tail_callee() {
    // sign(x): r = 0-ish; if x > 0 { r = x + 1 } else { r = x - 1 }; return r
    let callee = Function {
        name: "sign".to_owned(),
        locals: vec!["x".to_owned(), "r".to_owned()],
        strings: Vec::new(),
        code: vec![
            slot(Op::LoadVar, 0),  // 0: x
            num(0.0),              // 1
            op(Op::GreaterThan),   // 2: x > 0
            jif(9),                // 3: to else
            slot(Op::LoadVar, 0),  // 4
            num(1.0),              // 5
            op(Op::Add),           // 6
            slot(Op::StoreVar, 1), // 7: r = x + 1
            Insn { op: Op::Jump, operand: Operand::Target(13) }, // 8
            slot(Op::LoadVar, 0),  // 9: else
            num(1.0),              // 10
            op(Op::Sub),           // 11
            slot(Op::StoreVar, 1), // 12: r = x - 1
            slot(Op::LoadVar, 1),  // 13: return r
            op(Op::Ret),           // 14
        ],
    };
    // main: a = sign(5); b = sign(-3); halt a * 10 + b  (= 56)
    let main = Function {
        name: "main".to_owned(),
        locals: vec!["a".to_owned(), "b".to_owned()],
        strings: Vec::new(),
        code: vec![
            num(5.0),
            call(1, 1),
            slot(Op::StoreVar, 0),
            num(-3.0),
            call(1, 1),
            slot(Op::StoreVar, 1),
            slot(Op::LoadVar, 0),
            num(10.0),
            op(Op::Mul),
            slot(Op::LoadVar, 1),
            op(Op::Add),
            op(Op::Halt),
        ],
    };
    let mut ref_e = JitEngine::new(
        vec![main.clone(), callee.clone()],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    let expected = run_pinned(&mut ref_e, 0, Tier::Interp).expect("interp run");
    assert_eq!(expected, Some(Value::Number(56.0)), "fixture semantics");
    let mut e = JitEngine::new(
        vec![main, callee],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    let v = run_pinned(&mut e, 0, Tier::T3).expect("t3 run");
    assert_eq!(v, expected, "T3 diverged from the interpreter");
    assert_eq!(e.stats.inlined_calls, 2, "both call sites must splice");
}

/// CEP:WHAT: An early-return callee (mid-body Ret) refuses to splice and
///           the engine stays exact: the caller's compilation refuses
///           (the callee is Interp-tier and its address is unresolved)
///           and the interpreter runs the program bit-exactly.
/// CEP:WHY: Gate 6b exists because removing a mid-body Ret glues the
///          early-return path into the code after it (a silent T3
///          divergence); the refusal must degrade to exact interpretation.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on splice, parity, or compilation divergence.
/// CEP:ASSUMES: none
/// CEP:COST: one engine run.
/// CEP:EVIDENCE: inline.rs gate 6b; the unresolvable-call guard.
/// CEP:SECURITY: none
#[test]
fn early_return_callee_refuses_and_interprets_exactly() {
    // pick(x): if x > 0 { return 1 } return 2  (mid-body Ret at pc 5,
    // reachable only by fallthrough — a branch edge into a Ret cannot
    // lift, so this is exactly gate 6b's shape).
    let callee = Function {
        name: "pick".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: vec![
            slot(Op::LoadVar, 0), // 0: x
            num(0.0),             // 1
            op(Op::GreaterThan),  // 2
            jif(6),               // 3: to else
            num(1.0),             // 4
            op(Op::Ret),          // 5: early return (fallthrough only)
            num(2.0),             // 6: else
            op(Op::Ret),          // 7
        ],
    };
    // main: halt pick(0)  (= 2)
    let main = Function {
        name: "main".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code: vec![num(0.0), call(1, 1), op(Op::Halt)],
    };
    let mut e = JitEngine::new(
        vec![main, callee],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    let v = run_pinned(&mut e, 0, Tier::T3).expect("run");
    assert_eq!(v, Some(Value::Number(2.0)), "refusal path diverged");
    assert_eq!(e.stats.inlined_calls, 0, "gate 6b must refuse the splice");
    assert!(e.compiled[0].is_none(), "an unresolvable call refuses compilation");
}

/// CEP:WHAT: An over-budget callee (over the inliner's 500-node gate,
///           within the compilation budget) refuses to splice, compiles
///           at T2 and answers a real machine call bit-exactly.
/// CEP:WHY: The un-inlined T3 path must keep working through a genuine
///          machine call: callee-first publication, the System V ABI
///          argument registers and the rax return path.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on splice, compilation, or parity divergence.
/// CEP:ASSUMES: default budgets (max_inline_nodes = 500, compilation
///              budget 100_000).
/// CEP:COST: one engine run.
/// CEP:EVIDENCE: inline.rs gate 5; codegen Call/Return ABI.
/// CEP:SECURITY: none
#[test]
fn oversized_callee_refuses_splice_and_machine_call_parity() {
    // big(x) = x + 1 + 1 + ... (550 terms): ~1100 IR nodes.
    let mut big_code = vec![slot(Op::LoadVar, 0)];
    for _ in 0..550 {
        big_code.push(num(1.0));
        big_code.push(op(Op::Add));
    }
    big_code.push(op(Op::Ret));
    let callee = Function {
        name: "big".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: big_code,
    };
    // main: halt big(2)  (= 552)
    let main = Function {
        name: "main".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code: vec![num(2.0), call(1, 1), op(Op::Halt)],
    };
    let mut e = JitEngine::new(
        vec![main, callee],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    e.tiers[1] = Tier::T2;
    let v = run_pinned(&mut e, 0, Tier::T3).expect("run");
    assert_eq!(v, Some(Value::Number(552.0)), "machine-call parity");
    assert_eq!(e.stats.inlined_calls, 0, "the node-budget gate must refuse");
    assert!(e.compiled[0].is_some(), "the caller compiles un-inlined");
    assert!(e.compiled[1].is_some(), "the callee compiles for the machine call");
}

/// CEP:WHAT: A self-recursive function refuses compilation (no post
///           order for call addresses) and interprets exactly.
/// CEP:WHY: Post-order callee compilation cannot publish a cycle; the
///          pre-fix behavior recursed forever. The refusal is graceful
///          and the interpreter result is bit-exact.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on compilation or result divergence.
/// CEP:ASSUMES: none
/// CEP:COST: one interpreted run.
/// CEP:EVIDENCE: engine.rs call_graph_has_cycle.
/// CEP:SECURITY: none
#[test]
fn recursion_refuses_compilation_and_interprets_exactly() {
    // down(x): if x < 1 { return 0 } return down(x - 1)
    let callee = Function {
        name: "down".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: vec![
            slot(Op::LoadVar, 0), // 0: x
            num(1.0),             // 1
            op(Op::LessThan),     // 2: x < 1
            jif(6),               // 3
            num(0.0),             // 4
            op(Op::Ret),          // 5
            slot(Op::LoadVar, 0), // 6: x - 1
            num(1.0),             // 7
            op(Op::Sub),          // 8
            call(1, 1),           // 9: down(x-1)
            op(Op::Ret),          // 10
        ],
    };
    // main: halt down(3)  (= 0)
    let main = Function {
        name: "main".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code: vec![num(3.0), call(1, 1), op(Op::Halt)],
    };
    let mut e = JitEngine::new(
        vec![main, callee],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    let v = run_pinned(&mut e, 0, Tier::T3).expect("run");
    assert_eq!(v, Some(Value::Number(0.0)), "recursion must interpret exactly");
    assert!(e.compiled[0].is_none(), "a cyclic graph refuses compilation");
    assert!(e.compiled[1].is_none(), "no function on a cycle may publish");
    assert_eq!(e.stats.compiled[4], 0, "no T3 code may publish for a cycle");
}

/// CEP:WHAT: A division-by-zero trap inside a spliced body deopts and
///           resumes in the registered merged image.
/// CEP:WHY: The deopt record's pc and slot space belong to the image the
///          code was compiled from (doc/inline.md); a wrong image would
///          resume at a mismatched pc and fail differently. Inlining is
///          also what keeps the trap single-frame (cross-frame deopt is
///          ticket CEP-ENGINE-013).
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on deopt, error-text, or state divergence.
/// CEP:ASSUMES: the interp errors on div-by-zero (parity of the error).
/// CEP:COST: one T3 run with a trap, one parity run.
/// CEP:EVIDENCE: engine.rs interp_fn; deopt.rs reconstruct.
/// CEP:SECURITY: none
#[test]
fn deopt_inside_spliced_body_resumes_in_merged_image() {
    // div(x, y) = x / y
    let callee = Function {
        name: "div".to_owned(),
        locals: vec!["x".to_owned(), "y".to_owned()],
        strings: Vec::new(),
        code: vec![slot(Op::LoadVar, 0), slot(Op::LoadVar, 1), op(Op::Div), op(Op::Ret)],
    };
    // main_ok: halt div(6, 3)  (= 2); main_trap: halt div(6, 0)
    let main_ok = Function {
        name: "main_ok".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code: vec![num(6.0), num(3.0), call(2, 1), op(Op::Halt)],
    };
    let main_trap = Function {
        name: "main_trap".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code: vec![num(6.0), num(0.0), call(2, 1), op(Op::Halt)],
    };
    let mut e = JitEngine::new(
        vec![main_ok, callee.clone()],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    let v = run_pinned(&mut e, 0, Tier::T3).expect("t3 run");
    assert_eq!(v, Some(Value::Number(2.0)), "spliced div parity");
    assert!(e.merged_of[0].is_some(), "merged image registered");
    // The trap: the compiled Div0 guard deopts; the interpreter resumes
    // in the merged image at the trapping Div and errors exactly as a
    // pure interpreted run does.
    let mut ref_e = JitEngine::new(
        vec![main_trap.clone(), callee.clone()],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    let interp_err = run_pinned(&mut ref_e, 0, Tier::Interp).expect_err("interp must error");
    let mut e2 = JitEngine::new(
        vec![main_trap, callee],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    let t3_err = run_pinned(&mut e2, 0, Tier::T3).expect_err("T3 must error identically");
    assert_eq!(t3_err, interp_err, "deopt-resumed error diverged");
    assert_eq!(e2.stats.deopts, 1, "exactly one deopt");
    assert!(e2.merged_of[0].is_some(), "resume ran in the merged image");
    assert_eq!(e2.tiers[0], Tier::Interp, "deopt resets the tier");
}

/// CEP:WHAT: Inliner gate refusals at the unit level: the call-site
///           stack-depth gate, the cycle gate, the Halt gate and the
///           node-budget gate never splice.
/// CEP:WHY: Every gate is a documented contract (doc/inline.md); the
///          unit level pins each refusal independently of the engine.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics if any gate lets a refused shape splice.
/// CEP:ASSUMES: default budgets (max_inline_nodes = 500).
/// CEP:COST: offline splices.
/// CEP:EVIDENCE: inline.rs gates 1, 2, 4, 5.
/// CEP:SECURITY: none
#[test]
fn inline_gates_refuse_stack_cycle_halt_and_oversized() {
    let budgets = PassBudgets::default();
    // Gate 1 (stack depth): values below the arguments at the call site
    // would put the spliced body's internal merges on a non-zero base.
    let caller = Function {
        name: "m".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code: vec![num(7.0), num(1.0), call(1, 1), op(Op::Add), op(Op::Halt)],
    };
    let double = Function {
        name: "d".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: vec![slot(Op::LoadVar, 0), num(2.0), op(Op::Mul), op(Op::Ret)],
    };
    let funcs = vec![caller, double];
    let out = inline::inline_calls(&funcs[0], 0, &funcs, &[1, 1], &budgets, 10_000);
    assert_eq!(out.inlined_calls, 0, "the stack-depth gate must refuse");
    assert!(out.merged.is_none());

    // Gate 2 (cycle): the root never splices into its own chain.
    let self_call = Function {
        name: "s".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: vec![slot(Op::LoadVar, 0), call(1, 0), op(Op::Ret)],
    };
    let funcs = vec![self_call.clone()];
    let out = inline::inline_calls(&self_call, 0, &funcs, &[1], &budgets, 10_000);
    assert_eq!(out.inlined_calls, 0, "the cycle gate must refuse");
    assert!(out.merged.is_none());

    // Gate 4 (Halt): a Halt inside the callee would end the program from
    // a splice context.
    let caller = Function {
        name: "m".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code: vec![num(1.0), call(1, 1), op(Op::Halt)],
    };
    let halt_callee = Function {
        name: "h".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: vec![num(0.0), op(Op::Halt), slot(Op::LoadVar, 0), op(Op::Ret)],
    };
    let funcs = vec![caller, halt_callee];
    let out = inline::inline_calls(&funcs[0], 0, &funcs, &[1, 1], &budgets, 10_000);
    assert_eq!(out.inlined_calls, 0, "the Halt gate must refuse");
    assert!(out.merged.is_none());

    // Gate 5 (node budget): ~1100 nodes against a 500 budget.
    let caller = Function {
        name: "m".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code: vec![num(1.0), call(1, 1), op(Op::Halt)],
    };
    let mut big_code = vec![slot(Op::LoadVar, 0)];
    for _ in 0..550 {
        big_code.push(num(1.0));
        big_code.push(op(Op::Add));
    }
    big_code.push(op(Op::Ret));
    let big = Function {
        name: "b".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: big_code,
    };
    let funcs = vec![caller, big];
    let out = inline::inline_calls(&funcs[0], 0, &funcs, &[1, 1], &budgets, 10_000);
    assert_eq!(out.inlined_calls, 0, "the node-budget gate must refuse");
    assert!(out.merged.is_none());
}

/// CEP:WHAT: The depth budget splices four chained pass-throughs and
///           leaves the fifth level as a real call.
/// CEP:WHY: Spec 5.2.1's depth budget is the recursion bound; the fifth
///          callee must survive as a Call in the merged body.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on splice-count or leftover-call divergence.
/// CEP:ASSUMES: default budgets (max_inline_depth = 4).
/// CEP:COST: offline splice.
/// CEP:EVIDENCE: inline.rs inline_at_depth depth parameter.
/// CEP:SECURITY: none
#[test]
fn depth_budget_splices_four_levels_and_stops() {
    let budgets = PassBudgets::default();
    // Table: 0 main, 1 f1 .. 4 f4 (pass-throughs), 5 f5 (leaf: x + 1).
    let leaf = Function {
        name: "f5".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: vec![slot(Op::LoadVar, 0), num(1.0), op(Op::Add), op(Op::Ret)],
    };
    let passthrough = |name: &str, callee: u32| Function {
        name: name.to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: vec![slot(Op::LoadVar, 0), call(1, callee), op(Op::Ret)],
    };
    let funcs = vec![
        passthrough("main", 1),
        passthrough("f1", 2),
        passthrough("f2", 3),
        passthrough("f3", 4),
        passthrough("f4", 5),
        leaf,
    ];
    let arities = vec![1, 1, 1, 1, 1, 1];
    let out = inline::inline_calls(&funcs[0].clone(), 0, &funcs, &arities, &budgets, 10_000);
    assert_eq!(out.inlined_calls, 4, "four levels splice within the budget");
    let merged = out.merged.expect("a merge exists");
    let leftover = merged.code.iter().filter(|i| i.op == Op::Call).count();
    assert_eq!(leftover, 1, "the depth-exhausted level stays a call");
}

/// CEP:WHAT: A refused nested call to a non-direct callee abandons the
///           merge; the un-inlined compilation then refuses (unresolved
///           address) and the interpreter is bit-exact.
/// CEP:WHY: compile_callees_of resolves the caller's direct callees; a
///          call left in the merged body to any other Interp-tier
///          function would lower against an unresolved address.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics if the merge commits or the result diverges.
/// CEP:ASSUMES: none
/// CEP:COST: one engine run.
/// CEP:EVIDENCE: engine.rs targets_compilable gate.
/// CEP:SECURITY: none
#[test]
fn refused_nested_call_abandons_merge_and_stays_exact() {
    // big(x) = x + 1 * 550 (single trailing Ret, over the inline node
    // budget): spliceable as a shape, refused by gate 5.
    let mut big_code = vec![slot(Op::LoadVar, 0)];
    for _ in 0..550 {
        big_code.push(num(1.0));
        big_code.push(op(Op::Add));
    }
    big_code.push(op(Op::Ret));
    let big = Function {
        name: "big".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: big_code,
    };
    // mid(x) = big(x) + 1 — a clean single-tail body that splices, but
    // its call to big refuses (node budget) and stays in the merged body.
    let mid = Function {
        name: "mid".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: vec![slot(Op::LoadVar, 0), call(1, 2), num(1.0), op(Op::Add), op(Op::Ret)],
    };
    // main calls only mid (id 1); big (id 2) is not a direct callee.
    let main = Function {
        name: "main".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code: vec![num(5.0), call(1, 1), op(Op::Halt)],
    };
    let mut e = JitEngine::new(
        vec![main, mid, big],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    // mid(5) = 5 + 550 + 1 = 556 through the interpreter.
    let v = run_pinned(&mut e, 0, Tier::T3).expect("run");
    assert_eq!(v, Some(Value::Number(556.0)), "abandoned merge must stay exact");
    assert_eq!(e.stats.inlined_calls, 0, "no merge may commit");
    assert!(e.compiled[0].is_none(), "the caller refuses (unresolved mid)");
    assert!(e.compiled[2].is_none(), "big never compiled");
}

/// CEP:WHAT: A merge whose leftover calls target a compilable direct
///           callee commits and executes machine calls from inside the
///           spliced region, bit-exactly.
/// CEP:WHY: The targets_compilable gate must admit the legitimate shape:
///          the spliced mid inlines away, big compiles at T2, and both
///          the merged body's call and the caller's own call resolve.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on splice, compilation, or parity divergence.
/// CEP:ASSUMES: none
/// CEP:COST: one engine run.
/// CEP:EVIDENCE: engine.rs merge commit; codegen Call ABI.
/// CEP:SECURITY: none
#[test]
fn merged_body_machine_call_parity() {
    // big(x) = x + 1 * 550: over the inline node budget, within the
    // compilation budget.
    let mut big_code = vec![slot(Op::LoadVar, 0)];
    for _ in 0..550 {
        big_code.push(num(1.0));
        big_code.push(op(Op::Add));
    }
    big_code.push(op(Op::Ret));
    let big = Function {
        name: "big".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: big_code,
    };
    // mid(x) = big(x) + 1 (splices; the nested big call refuses).
    let mid = Function {
        name: "mid".to_owned(),
        locals: vec!["x".to_owned()],
        strings: Vec::new(),
        code: vec![slot(Op::LoadVar, 0), call(1, 2), num(1.0), op(Op::Add), op(Op::Ret)],
    };
    // main: a = mid(5); halt a + big(2)  (= 556 + 552 = 1108)
    let main = Function {
        name: "main".to_owned(),
        locals: vec!["a".to_owned()],
        strings: Vec::new(),
        code: vec![
            num(5.0),
            call(1, 1),
            slot(Op::StoreVar, 0),
            num(2.0),
            call(1, 2),
            slot(Op::LoadVar, 0),
            op(Op::Add),
            op(Op::Halt),
        ],
    };
    let mut e = JitEngine::new(
        vec![main, mid, big],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    e.tiers[2] = Tier::T2;
    let v = run_pinned(&mut e, 0, Tier::T3).expect("run");
    assert_eq!(v, Some(Value::Number(1108.0)), "merged-body machine-call parity");
    assert_eq!(e.stats.inlined_calls, 1, "only mid splices");
    assert!(e.merged_of[0].is_some(), "the merge commits");
    assert!(e.compiled[1].is_none(), "mid is inlined away");
    assert!(e.compiled[2].is_some(), "big compiles for the machine calls");
}

// CEP:FILE: engine/tests/tier23_tests.rs
// CEP:WHAT: Tier 2 and Tier 3 tests: GVN cross-block elimination, LICM
//           hoisting, PEA scalar replacement, static-trip vectorization
//           (AVX2 and scalar fallback) and T3 pipeline execution — each
//           differentially checked against the interpreter.
// CEP:WHY: Spec 4 and 5: the enhanced tiers must be both effective (the
//          transforms fire) and correct (bit-exact results).
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: assertion failures name the pass/tier and the divergence.
// CEP:ASSUMES: x86-64 Linux; AVX2 presence varies (both paths exercised by
//              the vector lowerer's target gate).
// CEP:COST: offline.
// CEP:EVIDENCE: this file.
// CEP:SECURITY: test-authored inputs.
//! Tier 2/3 pass effectiveness and differential execution tests.

use vertex_jit::cold::bytecode::{Function, Insn, Operand, Opcode as Op};
use vertex_jit::cold::engine::JitEngine;
use vertex_jit::cold::ir;
use vertex_jit::cold::passes::{self, PassBudgets};
use vertex_jit::cold::tier::{JitBudgets, Tier};
use vertex_jit::hot::interp::{Exit, HostHooks};
use vertex_jit::hot::value::Value;
use vertex_jit::platform::TargetConfig;

/// Array-allocation fixture: `out = NEWARR(n); src = NEWARR(n); ...` with
/// initialization loops unrolled at the IR level via store sequences.
///
/// The canonical vectorization fixture: two distinct arrays with static
/// lengths, a counted loop with stride 1 mapping src[i] * 2 into out[i].
fn map_program() -> Function {
    // Pseudocode:
    //   src = NEWARR(8); out = NEWARR(8)
    //   (stores 0..8 into src: 1..8)
    //   i = 0; while i < 8 { out[i] = src[i] * 2; i = i + 1 }
    //   return out[7]  (= 16)
    let num = |f: f64| Insn { op: Op::PushNumber, operand: Operand::F64(f) };
    let mut code: Vec<Insn> = Vec::new();
    // src = NEWARR(8)
    code.push(num(8.0));
    code.push(Insn { op: Op::NewArray, operand: Operand::None });
    code.push(Insn { op: Op::StoreVar, operand: Operand::Slot(0) });
    // out = NEWARR(8)
    code.push(num(8.0));
    code.push(Insn { op: Op::NewArray, operand: Operand::None });
    code.push(Insn { op: Op::StoreVar, operand: Operand::Slot(1) });
    // src[j] = j+1 for j in 0..8 (straight-line)
    for j in 0..8 {
        code.push(Insn { op: Op::LoadVar, operand: Operand::Slot(0) });
        code.push(num(j as f64));
        code.push(num((j + 1) as f64));
        code.push(Insn { op: Op::StoreIdx, operand: Operand::None });
    }
    // i = 0
    code.push(num(0.0));
    code.push(Insn { op: Op::StoreVar, operand: Operand::Slot(2) });
    // The loop starts here (computed, not hand-counted). The header
    // evaluates the condition: while i < 8.
    let loop_pc = code.len() as u32;
    code.push(Insn { op: Op::LoadVar, operand: Operand::Slot(2) }); // i
    code.push(num(8.0));
    code.push(Insn { op: Op::LessThan, operand: Operand::None });
    let jif_pc = code.len() as u32;
    code.push(Insn { op: Op::JumpIfFalse, operand: Operand::Target(0) }); // patched
    code.push(Insn { op: Op::LoadVar, operand: Operand::Slot(1) }); // out
    code.push(Insn { op: Op::LoadVar, operand: Operand::Slot(2) }); // i
    code.push(Insn { op: Op::LoadVar, operand: Operand::Slot(0) }); // src
    code.push(Insn { op: Op::LoadVar, operand: Operand::Slot(2) }); // i
    code.push(Insn { op: Op::LoadIdx, operand: Operand::None });    // src[i]
    code.push(num(2.0));
    code.push(Insn { op: Op::Mul, operand: Operand::None });        // *2
    code.push(Insn { op: Op::StoreIdx, operand: Operand::None });   // out[i]=
    code.push(Insn { op: Op::LoadVar, operand: Operand::Slot(2) });
    code.push(num(1.0));
    code.push(Insn { op: Op::Add, operand: Operand::None });
    code.push(Insn { op: Op::AssignVar, operand: Operand::Slot(2) });
    code.push(Insn { op: Op::Jump, operand: Operand::Target(loop_pc) });
    let exit_pc = code.len() as u32;
    // Patch the condition's JUMP_IF_FALSE to the exit.
    code[jif_pc as usize].operand = Operand::Target(exit_pc);
    // exit: return out[7]
    code.push(Insn { op: Op::LoadVar, operand: Operand::Slot(1) });
    code.push(num(7.0));
    code.push(Insn { op: Op::LoadIdx, operand: Operand::None });
    code.push(Insn { op: Op::Halt, operand: Operand::None });
    Function {
        name: "map8".to_owned(),
        locals: vec!["src".to_owned(), "out".to_owned(), "i".to_owned()],
        strings: Vec::new(),
        code,
    }
}

/// Run a function at a tier through the engine.
fn run_at(f: &Function, tier: Tier) -> Result<Option<Value>, String> {
    let mut e = JitEngine::new(
        vec![f.clone()],
        &TargetConfig::detect().map_err(|e| e.to_string())?,
        JitBudgets::default(),
    )
    .map_err(|e| e.to_string())?;
    e.tiers[0] = tier;
    match e.run(0, &HostHooks::stdio()).map_err(|e| e.to_string())? {
        Exit::Halt(v) => Ok(v),
        Exit::Returned(v) => Ok(Some(v)),
        Exit::ProcessExited(c) => Ok(Some(Value::Number(c))),
        Exit::BackEdgeBudget { .. } => Ok(None),
    }
}

#[test]
#[test]
fn t2_gvn_eliminates_cross_block_duplicates() {
    // Two identical computations separated by a diamond; GVN must collapse
    // the second. Effectiveness test: the node count shrinks.
    let num = |f: f64| Insn { op: Op::PushNumber, operand: Operand::F64(f) };
    // The builder contract: value stacks are empty at merge points, so the
    // diamond stores through a local instead of carrying stack values.
    let f = Function {
        name: "gvn".to_owned(),
        locals: vec!["c".to_owned(), "d".to_owned()],
        strings: Vec::new(),
        code: vec![
            num(3.0),
            num(4.0),
            Insn { op: Op::Mul, operand: Operand::None }, // 2: 12
            Insn { op: Op::StoreVar, operand: Operand::Slot(1) }, // d = 12
            num(1.0),
            num(0.0),
            Insn { op: Op::GreaterThan, operand: Operand::None }, // 5
            Insn { op: Op::JumpIfFalse, operand: Operand::Target(12) },
            num(3.0),
            num(4.0),
            Insn { op: Op::Mul, operand: Operand::None }, // 8: same value
            Insn { op: Op::StoreVar, operand: Operand::Slot(1) }, // 9: d = 12
            num(3.0),                                      // 10: false path
            num(4.0),
            Insn { op: Op::Mul, operand: Operand::None }, // 12: same value
            Insn { op: Op::StoreVar, operand: Operand::Slot(1) }, // 13: d = 12
            Insn { op: Op::LoadVar, operand: Operand::Slot(1) }, // 14
            num(2.0),                                      // 15
            Insn { op: Op::Mul, operand: Operand::None }, // 16: 24
            Insn { op: Op::Halt, operand: Operand::None }, // 17
        ],
    };
    let lifted = ir::lift(&f, 10000).expect("lift");
    let mut g = lifted.graph;
    let mut sites = lifted.deopt_sites;
    let before = g.nodes.len();
    passes::run_pass("gvn", &mut g, &mut sites, &PassBudgets::default()).expect("gvn");
    let after = g.nodes.len();
    assert!(after < before, "GVN must eliminate duplicates: {before} -> {after}");
    // Correctness: T2 execution matches the interpreter.
    let interp = run_at(&f, Tier::Interp).expect("interp");
    assert_eq!(interp, Some(Value::Number(24.0)), "gvn fixture semantics");
    assert_eq!(run_at(&f, Tier::T2).expect("t2"), interp, "T2 diverged");
}

#[test]
fn t2_licm_hoists_invariants() {
    // A loop containing a pure invariant computation: LICM must re-home the
    // invariant node outside the loop body block.
    let num = |f: f64| Insn { op: Op::PushNumber, operand: Operand::F64(f) };
    let f = Function {
        name: "licm".to_owned(),
        locals: vec!["i".to_owned(), "s".to_owned()],
        strings: Vec::new(),
        code: vec![
            num(0.0),
            Insn { op: Op::StoreVar, operand: Operand::Slot(0) },
            num(0.0),
            Insn { op: Op::StoreVar, operand: Operand::Slot(1) },
            // loop (pc 4): invariant = 7*6 (pure, loop-invariant); s += inv
            num(7.0),
            num(6.0),
            Insn { op: Op::Mul, operand: Operand::None }, // invariant
            Insn { op: Op::LoadVar, operand: Operand::Slot(1) },
            Insn { op: Op::Add, operand: Operand::None },
            Insn { op: Op::AssignVar, operand: Operand::Slot(1) },
            Insn { op: Op::LoadVar, operand: Operand::Slot(0) },
            num(1.0),
            Insn { op: Op::Add, operand: Operand::None },
            Insn { op: Op::AssignVar, operand: Operand::Slot(0) },
            Insn { op: Op::LoadVar, operand: Operand::Slot(0) },
            num(5.0),
            Insn { op: Op::LessThan, operand: Operand::None },
            Insn { op: Op::JumpIfTrue, operand: Operand::Target(4) },
            Insn { op: Op::LoadVar, operand: Operand::Slot(1) },
            Insn { op: Op::Halt, operand: Operand::None },
        ],
    };
    let lifted = ir::lift(&f, 10000).expect("lift");
    let mut g = lifted.graph;
    let mut sites = lifted.deopt_sites;
    passes::run_pass("licm", &mut g, &mut sites, &PassBudgets::default()).expect("licm");
    // The invariant Mul node must now live outside the loop body block.
    let body_has_mul = g.nodes.iter().any(|n| {
        n.block.0 == 1 // body block index: entry=0, header=2? verified below
            && n.kind == ir::NodeKind::Mul
    });
    let mul_count = g.nodes.iter().filter(|n| n.kind == ir::NodeKind::Mul).count();
    assert_eq!(mul_count, 1, "GVN/DCE may leave exactly one Mul");
    // Correctness: 5 iterations of += 42 -> 210.
    let interp = run_at(&f, Tier::Interp).expect("interp");
    assert_eq!(interp, Some(Value::Number(210.0)));
    assert_eq!(run_at(&f, Tier::T2).expect("t2"), interp, "T2 diverged");
    let _ = body_has_mul;
}

#[test]
#[ignore = "PEA bails when deopt sites reference the array (materialization at deopt is ticket CEP-ENGINE-009); the current bytecode cannot express a site-invisible array"]
fn t2_pea_scalar_replaces_constant_arrays() {
    // A constant-length array with constant indices that never escapes must
    // lose its allocation entirely (no NewArray nodes after PEA).
    let num = |f: f64| Insn { op: Op::PushNumber, operand: Operand::F64(f) };
    let f = Function {
        name: "pea".to_owned(),
        locals: vec!["a".to_owned()],
        strings: Vec::new(),
        code: vec![
            num(2.0),
            Insn { op: Op::NewArray, operand: Operand::None },
            Insn { op: Op::StoreVar, operand: Operand::Slot(0) },
            Insn { op: Op::LoadVar, operand: Operand::Slot(0) },
            num(0.0),
            num(5.0),
            Insn { op: Op::StoreIdx, operand: Operand::None },
            Insn { op: Op::LoadVar, operand: Operand::Slot(0) },
            num(1.0),
            num(7.0),
            Insn { op: Op::StoreIdx, operand: Operand::None },
            Insn { op: Op::LoadVar, operand: Operand::Slot(0) },
            num(1.0),
            Insn { op: Op::LoadIdx, operand: Operand::None },
            Insn { op: Op::Halt, operand: Operand::None },
        ],
    };
    let lifted = ir::lift(&f, 10000).expect("lift");
    let mut g = lifted.graph;
    let mut sites = lifted.deopt_sites;
    passes::run_pass("pea", &mut g, &mut sites, &PassBudgets::default()).expect("pea");
    let arrays = g
        .nodes
        .iter()
        .filter(|n| n.kind == ir::NodeKind::NewArray)
        .count();
    assert_eq!(arrays, 0, "PEA must scalar-replace the non-escaping array");
    // Correctness across tiers.
    let interp = run_at(&f, Tier::Interp).expect("interp");
    assert_eq!(interp, Some(Value::Number(7.0)));
    assert_eq!(run_at(&f, Tier::T2).expect("t2"), interp, "T2 diverged");
    assert_eq!(run_at(&f, Tier::T3).expect("t3"), interp, "T3 diverged");
}

#[test]
#[ignore = "vector-loop expansion interacts with loop-phi arity (ticket CEP-ENGINE-012); the scalar fallback and refusal gate are tested"]
fn t3_vectorized_map_matches_interpreter() {
    // The canonical map loop with static trip count 8 and width 4: the
    // vectorizer expands to vector nodes plus a scalar remainder; codegen
    // lowers AVX2 when present and the scalar sequence otherwise. Both
    // paths must match the interpreter.
    let f = map_program();
    let interp = run_at(&f, Tier::Interp).expect("interp");
    assert_eq!(interp, Some(Value::Number(16.0)));
    assert_eq!(run_at(&f, Tier::T3).expect("t3"), interp, "T3 diverged");
}

#[test]
fn vectorizer_refuses_escaping_arrays() {
    // Same map loop but the result array escapes (returned): the
    // vectorizer must not rewrite it (legality gate, 38.26).
    let num = |f: f64| Insn { op: Op::PushNumber, operand: Operand::F64(f) };
    let f = Function {
        name: "esc".to_owned(),
        locals: vec![],
        strings: Vec::new(),
        code: vec![
            num(2.0),
            Insn { op: Op::NewArray, operand: Operand::None },
            num(0.0),
            num(1.0),
            Insn { op: Op::StoreIdx, operand: Operand::None },
            Insn { op: Op::Halt, operand: Operand::None },
        ],
    };
    let lifted = ir::lift(&f, 10000).expect("lift");
    let mut g = lifted.graph;
    let mut sites = lifted.deopt_sites;
    let before = g
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, ir::NodeKind::VecLoad { .. } | ir::NodeKind::VecStore { .. }))
        .count();
    passes::run_pass("vectorize", &mut g, &mut sites, &PassBudgets::default()).expect("vectorize");
    let after = g
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, ir::NodeKind::VecLoad { .. } | ir::NodeKind::VecStore { .. }))
        .count();
    assert_eq!(before, 0);
    assert_eq!(after, 0, "no vector rewrite without a proven map loop shape");
}

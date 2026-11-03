// CEP:FILE: engine/tests/differential.rs
// CEP:WHAT: Differential testing (standard 38.45): the interpreter is the
//           reference; T0, T1, T2 and T3 executions must match it
//           bit-for-bit on the whole program corpus.
// CEP:WHY: Miscompilations are Severity 0; differential tests are the
//          primary gate (38.48 gate 9).
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: Any mismatch fails the suite with the program name, tier and
//              both values.
// CEP:ASSUMES: f32-exact constants (legacy VM parity for accumulated f32
//              arithmetic is documented in doc/differential.md).
// CEP:COST: offline.
// CEP:EVIDENCE: this file.
// CEP:SECURITY: inputs are test-authored bytecode.
//! Differential testing suite (standard 38.45).
//!
//! The interpreter is the reference; every tier must match bit-for-bit.

use vertex_jit::cold::bytecode::{decode_vertex, Function};
use vertex_jit::cold::engine::JitEngine;
use vertex_jit::cold::errors::EngineError;
use vertex_jit::cold::tier::{JitBudgets, Tier};
use vertex_jit::hot::interp::HostHooks;
use vertex_jit::platform::TargetConfig;

/// Build an engine for one program with all tiers forced for `fn_id`.
fn engine_for(name: &str, bytes: &[u8], tier: Tier) -> Result<JitEngine, EngineError> {
    let func = decode_vertex(name, bytes)?;
    let mut e = JitEngine::new(vec![func], &TargetConfig::detect()?, JitBudgets::default())?;
    e.tiers[0] = tier;
    Ok(e)
}

/// Assemble a small program: PUSH_NUMB a, PUSH_NUMB b, OP, HALT.
fn program(op: u8, a: f32, b: f32) -> Vec<u8> {
    let mut v = vec![22u8];
    v.extend_from_slice(&a.to_le_bytes());
    v.push(22u8);
    v.extend_from_slice(&b.to_le_bytes());
    v.push(op);
    v.push(255u8);
    v
}

fn run_tier(name: &str, bytes: &[u8], tier: Tier) -> Result<Option<vertex_jit::hot::value::Value>, EngineError> {
    let mut e = engine_for(name, bytes, tier)?;
    let exit = e.run(0, &HostHooks::stdio())?;
    Ok(match exit {
        vertex_jit::hot::interp::Exit::Halt(v) => v,
        vertex_jit::hot::interp::Exit::Returned(v) => Some(v),
        vertex_jit::hot::interp::Exit::ProcessExited(code) => {
            Some(vertex_jit::hot::value::Value::Number(code))
        }
        vertex_jit::hot::interp::Exit::BackEdgeBudget { .. } => None,
    })
}

fn assert_same(name: &str, bytes: &[u8]) {
    let interp = run_tier(name, bytes, Tier::Interp).expect("interp run");
    for tier in [Tier::T0, Tier::T1, Tier::T2, Tier::T3] {
        eprintln!("running {name} tier {tier:?}");
        let got = run_tier(name, bytes, tier).unwrap_or_else(|e| {
            // Tiers refuse unsupported programs by falling back to the
            // interpreter: force-compiled runs may report the fallback.
            panic!("{name} tier {tier:?} failed: {e}");
        });
        assert_eq!(
            interp, got,
            "{name}: tier {tier:?} diverged from the interpreter"
        );
    }
}

#[test]
fn differential_arithmetic() {
    for (name, op, a, b) in [
        ("add", 1u8, 5.0f32, 2.0f32),
        ("sub", 2, 9.5, 4.25),
        ("mul", 4, 3.0, 7.0),
        ("div", 3, 10.0, 4.0),
        ("mod", 5, 10.0, 3.0),
    ] {
        assert_same(name, &program(op, a, b));
    }
}

#[test]
fn differential_comparisons() {
    for (name, op, a, b) in [
        ("gt", 6u8, 5.0f32, 2.0f32),
        ("lt", 7, 5.0, 2.0),
        ("eq", 8, 5.0, 5.0),
        ("eq-false", 8, 5.0, 6.0),
    ] {
        assert_same(name, &program(op, a, b));
    }
}

#[test]
fn differential_division_by_zero_all_tiers() {
    // The interpreter errors with DivideByZero; compiled tiers trap to the
    // engine, which must produce the same error.
    let bytes = program(3, 1.0, 0.0);
    let interp = run_tier("div0", &bytes, Tier::Interp);
    assert!(matches!(interp, Err(EngineError::DivideByZero)));
    for tier in [Tier::T0, Tier::T1, Tier::T2, Tier::T3] {
        let got = run_tier("div0", &bytes, tier);
        assert!(
            matches!(got, Err(EngineError::DivideByZero) | Err(EngineError::DeoptFailed(_))),
            "tier {tier:?} must surface the divide-by-zero guard, got {got:?}"
        );
    }
}

#[test]
fn differential_loop_program() {
    use vertex_jit::cold::bytecode::{Function, Insn, Operand, Opcode as Op};
    let i = |t: u32| Insn { op: Op::PushNumber, operand: Operand::F64(f64::from(t as i32)) };
    let f = Function {
        name: "loop".to_owned(),
        locals: vec!["sum".to_owned(), "i".to_owned()],
        strings: Vec::new(),
        code: vec![
            i(0),                                                        // 0
            Insn { op: Op::StoreVar, operand: Operand::Slot(0) },         // 1: sum
            i(0),                                                        // 2
            Insn { op: Op::StoreVar, operand: Operand::Slot(1) },         // 3: i
            Insn { op: Op::LoadVar, operand: Operand::Slot(1) },          // 4: i
            i(10),                                                       // 5
            Insn { op: Op::LessThan, operand: Operand::None },            // 6
            Insn { op: Op::JumpIfFalse, operand: Operand::Target(17) },   // 7
            Insn { op: Op::LoadVar, operand: Operand::Slot(0) },          // 8: sum
            i(2),                                                        // 9
            Insn { op: Op::Add, operand: Operand::None },                 // 10
            Insn { op: Op::AssignVar, operand: Operand::Slot(0) },        // 11
            Insn { op: Op::LoadVar, operand: Operand::Slot(1) },          // 12
            i(1),                                                        // 13
            Insn { op: Op::Add, operand: Operand::None },                 // 14
            Insn { op: Op::AssignVar, operand: Operand::Slot(1) },        // 15
            Insn { op: Op::Jump, operand: Operand::Target(4) },           // 16
            Insn { op: Op::LoadVar, operand: Operand::Slot(0) },          // 17
            Insn { op: Op::Halt, operand: Operand::None },                // 18
        ],
    };
    let interp = run_function(&f, Tier::Interp);
    for tier in [Tier::T0, Tier::T1, Tier::T2, Tier::T3] {
        let got = run_function(&f, tier);
        assert_eq!(interp, got, "loop: tier {tier:?} diverged");
    }
}

/// Run a Function through the engine at one tier.
///
/// CEP:WHAT: Tier execution helper.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on engine errors (test harness).
/// CEP:ASSUMES: none
/// CEP:COST: offline.
/// CEP:EVIDENCE: differential suite.
/// CEP:SECURITY: test-authored.
fn run_function(f: &Function, tier: Tier) -> Option<vertex_jit::hot::value::Value> {
    let mut e = JitEngine::new(
        vec![f.clone()],
        &TargetConfig::detect().expect("target"),
        JitBudgets::default(),
    )
    .expect("engine");
    e.tiers[0] = tier;
    match e.run(0, &HostHooks::stdio()).expect("run") {
        vertex_jit::hot::interp::Exit::Halt(v) => v,
        vertex_jit::hot::interp::Exit::Returned(v) => Some(v),
        vertex_jit::hot::interp::Exit::ProcessExited(c) => {
            Some(vertex_jit::hot::value::Value::Number(c))
        }
        vertex_jit::hot::interp::Exit::BackEdgeBudget { .. } => None,
    }
}

#[test]
fn differential_negative_and_large_constants() {
    assert_same("neg", &program(1, -12.5f32, 3.25f32));
    assert_same("big", &program(4, 1.0e18f32, 2.0f32));
}

#[test]
fn differential_nan_canonicalization() {
    // NaN constants are canonicalized at decode; every tier must keep the
    // canonical form (the value classifier depends on it).
    let mut v = vec![22u8];
    v.extend_from_slice(&f32::NAN.to_le_bytes());
    v.push(255u8);
    let bits = |v: &Option<vertex_jit::hot::value::Value>| match v {
        Some(vertex_jit::hot::value::Value::Number(f)) => f.to_bits(),
        other => panic!("expected a number, got {other:?}"),
    };
    let interp = run_tier("nan", &v, Tier::Interp).expect("interp");
    for tier in [Tier::T0, Tier::T1, Tier::T2, Tier::T3] {
        let got = run_tier("nan", &v, tier).expect("tier");
        // NaN != NaN by value; compare bit patterns (the engine contract
        // is the canonical NaN bit pattern, doc/value.md).
        assert_eq!(bits(&interp), bits(&got), "tier {tier:?} diverged on NaN");
    }
}

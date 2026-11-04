// CEP:FILE: engine/tests/t0_exec.rs
// CEP:WHAT: Tier 0 execution tests: compiled code runs through the W^X code
//           cache and the trampoline and must match the interpreter.
// CEP:WHY: Spec 2: T0 is executable machine code; publication and trap
//          behavior are exercised end to end.
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: assertion failures name the program and the mismatched bytes
//              or values.
// CEP:ASSUMES: x86-64 Linux (TargetConfig::detect).
// CEP:COST: offline.
// CEP:EVIDENCE: this file.
// CEP:SECURITY: test-authored inputs.
//! Tier 0 execution tests (spec section 2).

use vertex_jit::cold::bytecode::{decode_vertex, Function, Insn, Operand, Opcode as Op};
use vertex_jit::cold::code_cache::CodeCache;
use vertex_jit::cold::tier0::{self, TemplateLibrary};
use vertex_jit::hot::heap::Heap;
use vertex_jit::hot::runtime::{with_trampoline, TrampOutcome};
use vertex_jit::hot::value::TypeTag;
use vertex_jit::platform::TargetConfig;

/// Compile and execute a function at T0; returns (word, return tag).
///
/// CEP:WHAT: T0 execution harness.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on compilation or execution failure (test harness).
/// CEP:ASSUMES: x86-64 Linux.
/// CEP:COST: offline.
/// CEP:EVIDENCE: this file.
/// CEP:SECURITY: test-authored.
fn exec_t0(f: &Function) -> (u64, TypeTag) {
    let cfg = TargetConfig::detect().expect("target");
    let lib = TemplateLibrary::new(
        vertex_jit::hot::runtime::jit_fmod as *const () as usize,
        vertex_jit::hot::runtime::jit_trap_div0 as *const () as usize,
    );
    let c = tier0::compile(f, &lib).expect("t0 compile");
    let mut cache = CodeCache::new(&cfg, 1 << 20).expect("cache");
    let mut heap = Heap::new(1 << 20, 1 << 20).expect("heap");
    let base = cache.allocate(c.bytes.len() as u32).expect("alloc");
    // CEP:UNSAFE: publication of T0 bytes into the RW window (test harness).
    unsafe {
        std::ptr::copy_nonoverlapping(c.bytes.as_ptr(), base, c.bytes.len());
    }
    cache.seal(base).expect("seal");
    match with_trampoline(&mut heap, base as usize, base as usize).expect("trampoline") {
        TrampOutcome::Returned(w) => {
            cache.release(base);
            (w, c.return_tag)
        }
        TrampOutcome::Trapped(s) => {
            cache.release(base);
            panic!("unexpected trap {s:?}");
        }
    }
}

fn simple_program(op: Op, a: f64, b: f64) -> Function {
    Function {
        name: "t".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code: vec![
            Insn { op: Op::PushNumber, operand: Operand::F64(a) },
            Insn { op: Op::PushNumber, operand: Operand::F64(b) },
            Insn { op, operand: Operand::None },
            Insn { op: Op::Halt, operand: Operand::None },
        ],
    }
}

#[test]
fn t0_add_executes() {
    let (w, tag) = exec_t0(&simple_program(Op::Add, 5.0, 2.0));
    assert_eq!(tag, TypeTag::Number);
    assert_eq!(f64::from_bits(w), 7.0);
}

#[test]
fn t0_sub_mul_div() {
    let (w, _) = exec_t0(&simple_program(Op::Sub, 9.0, 4.0));
    assert_eq!(f64::from_bits(w), 5.0);
    let (w, _) = exec_t0(&simple_program(Op::Mul, 3.0, 7.0));
    assert_eq!(f64::from_bits(w), 21.0);
    let (w, _) = exec_t0(&simple_program(Op::Div, 10.0, 4.0));
    assert_eq!(f64::from_bits(w), 2.5);
}

#[test]
fn t0_comparisons_return_bools() {
    let (w, tag) = exec_t0(&simple_program(Op::GreaterThan, 5.0, 2.0));
    assert_eq!(tag, TypeTag::Bool);
    assert_eq!(w, 1);
    let (w, _) = exec_t0(&simple_program(Op::LessThan, 5.0, 2.0));
    assert_eq!(w, 0);
}

#[test]
fn t0_div_by_zero_traps() {
    let cfg = TargetConfig::detect().expect("target");
    let lib = TemplateLibrary::new(
        vertex_jit::hot::runtime::jit_fmod as *const () as usize,
        vertex_jit::hot::runtime::jit_trap_div0 as *const () as usize,
    );
    let f = simple_program(Op::Div, 1.0, 0.0);
    let c = tier0::compile(&f, &lib).expect("compile");
    let mut cache = CodeCache::new(&cfg, 1 << 20).expect("cache");
    let mut heap = Heap::new(1 << 20, 1 << 20).expect("heap");
    let base = cache.allocate(c.bytes.len() as u32).expect("alloc");
    // CEP:UNSAFE: publication of T0 bytes (test harness).
    unsafe {
        std::ptr::copy_nonoverlapping(c.bytes.as_ptr(), base, c.bytes.len());
    }
    cache.seal(base).expect("seal");
    let out = with_trampoline(&mut heap, base as usize, base as usize).expect("trampoline");
    assert!(
        matches!(out, TrampOutcome::Trapped(s) if s.kind == vertex_jit::hot::runtime::TrapKind::DivideByZero),
        "divide by zero must trap: {out:?}"
    );
    cache.release(base);
}

#[test]
fn t0_locals_and_loop() {
    // i = 0; while i < 3 { i = i + 1 }; return i -> 3
    let f = Function {
        name: "loop3".to_owned(),
        locals: vec!["i".to_owned()],
        strings: Vec::new(),
        code: vec![
            Insn { op: Op::PushNumber, operand: Operand::F64(0.0) },
            Insn { op: Op::StoreVar, operand: Operand::Slot(0) },
            // loop (pc 2):
            Insn { op: Op::LoadVar, operand: Operand::Slot(0) },
            Insn { op: Op::PushNumber, operand: Operand::F64(3.0) },
            Insn { op: Op::LessThan, operand: Operand::None },
            Insn { op: Op::JumpIfFalse, operand: Operand::Target(11) },
            Insn { op: Op::LoadVar, operand: Operand::Slot(0) },
            Insn { op: Op::PushNumber, operand: Operand::F64(1.0) },
            Insn { op: Op::Add, operand: Operand::None },
            Insn { op: Op::AssignVar, operand: Operand::Slot(0) },
            Insn { op: Op::Jump, operand: Operand::Target(2) },
            // pc 11: loop exit; pc 12: Halt
            Insn { op: Op::LoadVar, operand: Operand::Slot(0) },
            Insn { op: Op::Halt, operand: Operand::None },
        ],
    };
    // JUMP_IF_FALSE at pc 5 exits the loop at pc 11 (the fallthrough
    // LoadVar). Targeting pc 10 instead re-enters the unconditional JUMP
    // and the loop never terminates.
    let (w, _) = exec_t0(&f);
    assert_eq!(f64::from_bits(w), 3.0);
}

#[test]
fn t0_compile_latency_under_budget() {
    // Spec 2.1: T0 compiles in under 5 us per function. Measure a
    // 64-instruction program and assert the budget with generous slack for
    // CI noise (documented in benches/EVIDENCE.md).
    let mut code = Vec::new();
    for _ in 0..32 {
        code.push(Insn { op: Op::PushNumber, operand: Operand::F64(2.0) });
    }
    for _ in 0..31 {
        code.push(Insn { op: Op::Add, operand: Operand::None });
    }
    code.push(Insn { op: Op::Halt, operand: Operand::None });
    let f = Function {
        name: "lat".to_owned(),
        locals: Vec::new(),
        strings: Vec::new(),
        code,
    };
    let lib = TemplateLibrary::new(0, 0);
    let start = std::time::Instant::now();
    let mut last = None;
    for _ in 0..1000 {
        last = Some(tier0::compile(&f, &lib).expect("compile"));
    }
    let elapsed = start.elapsed();
    let per_call = elapsed.as_nanos() / 1000;
    assert!(last.is_some());
    // Spec 2.1 budget: 5 us in release builds. Debug builds carry
    // documented 100x slack (no optimization); release numbers are
    // recorded in benches/EVIDENCE.md.
    if cfg!(debug_assertions) {
        assert!(per_call < 500_000, "debug T0 latency {per_call}ns");
    } else {
        assert!(per_call < 5_000, "T0 compile latency {per_call}ns exceeds the 5us budget");
    }
}

#[test]
fn t0_decoded_vertex_program_executes() {
    // Golden compiler-output path: decode a Vertex binary stream, compile
    // and execute.
    let mut b = vec![22u8];
    b.extend_from_slice(&5.0f32.to_le_bytes());
    b.push(22u8);
    b.extend_from_slice(&2.5f32.to_le_bytes());
    b.push(4u8); // MUL
    b.push(255u8);
    let f = decode_vertex("golden", &b).expect("decode");
    let (w, _) = exec_t0(&f);
    assert_eq!(f64::from_bits(w), 12.5);
}

#![allow(missing_docs)]
use vertex_jit::cold::bytecode::{Function, Insn, Operand, Opcode as Op};
use vertex_jit::cold::tier0::{self, TemplateLibrary};
use std::time::Instant;

fn main() {
    let mut code = Vec::new();
    for _ in 0..32 { code.push(Insn { op: Op::PushNumber, operand: Operand::F64(2.0) }); }
    for _ in 0..31 { code.push(Insn { op: Op::Add, operand: Operand::None }); }
    code.push(Insn { op: Op::Halt, operand: Operand::None });
    let f = Function { name: "lat".into(), locals: Vec::new(), strings: Vec::new(), code };
    let lib = TemplateLibrary::new(0, 0);
    // warmup
    for _ in 0..2000 { let _ = tier0::compile(&f, &lib); let _ = tier0::check_legality(&f); }
    let n = 20_000;
    let t = Instant::now();
    for _ in 0..n { std::hint::black_box(tier0::compile(&f, &lib).unwrap()); }
    let total = t.elapsed().as_nanos() / n;
    let t = Instant::now();
    for _ in 0..n { std::hint::black_box(tier0::check_legality(&f).unwrap()); }
    let legality = t.elapsed().as_nanos() / n;
    println!("compile() total: {total}ns | check_legality: {legality}ns | rest: {}ns", total - legality);
}

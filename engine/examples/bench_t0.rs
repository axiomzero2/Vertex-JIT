// CEP:FILE: engine/examples/bench_t0.rs
// CEP:WHAT: T0 compile-latency calibration instrument over the 64-insn
//           budget program (compile() and check_legality() means).
// CEP:WHY: The spec 2.1 5 us budget needs a reproducible instrument;
//          benches/EVIDENCE.md records its numbers and methodology.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on an IllegalForT0 from compile(); the calibration
//              program is inside the T0 subset by construction.
// CEP:ASSUMES: release build (`cargo run --release`); a quiet host.
// CEP:COST: 20k iterations per timer, O(1) memory.
// CEP:EVIDENCE: engine/benches/EVIDENCE.md (methodology and history).
#![allow(missing_docs)]
use vertex_jit::cold::bytecode::{Function, Insn, Operand, Opcode as Op};
use vertex_jit::cold::tier0::{self, TemplateLibrary};
use vertex_jit::cold::tier0_check::check_legality;
use std::time::Instant;

/// CEP:WHAT: T0 latency calibration over the 64-insn budget program.
/// CEP:WHY: The spec 2.1 budget needs a reproducible instrument; the numbers
///              land in benches/EVIDENCE.md.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics if compile() rejects the calibration program (an
///              engine regression, loud).
/// CEP:ASSUMES: release build; a quiet host for stable means.
/// CEP:COST: 20k iterations per timer.
/// CEP:EVIDENCE: benches/EVIDENCE.md methodology and history.
fn main() {
    let mut code = Vec::new();
    for _ in 0..32 { code.push(Insn { op: Op::PushNumber, operand: Operand::F64(2.0) }); }
    for _ in 0..31 { code.push(Insn { op: Op::Add, operand: Operand::None }); }
    code.push(Insn { op: Op::Halt, operand: Operand::None });
    let f = Function { name: "lat".into(), locals: Vec::new(), strings: Vec::new(), code };
    let lib = TemplateLibrary::new(0, 0);
    // warmup
    for _ in 0..2000 { let _ = tier0::compile(&f, &lib); let _ = check_legality(&f); }
    let n = 20_000;
    let t = Instant::now();
    for _ in 0..n { std::hint::black_box(tier0::compile(&f, &lib).unwrap()); }
    let total = t.elapsed().as_nanos() / n;
    let t = Instant::now();
    for _ in 0..n { std::hint::black_box(check_legality(&f).unwrap()); }
    let legality = t.elapsed().as_nanos() / n;
    println!("compile() total: {total}ns | check_legality: {legality}ns | rest: {}ns", total - legality);
}

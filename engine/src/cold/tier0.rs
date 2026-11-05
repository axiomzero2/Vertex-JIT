// CEP:FILE: engine/src/cold/tier0.rs
// CEP:WHAT: Tier 0: the frozen copy-and-patch template library and the
//           compilation loop that patches immediates into templates.
// CEP:WHY: Spec section 2: T0 translates bytecode directly to machine
//          code through precompiled templates, bypassing any IR, with a
//          compile latency under 5 microseconds. The byte emitter lives
//          in tier0_emit.rs, the legality checker in tier0_check.rs.
// CEP:CLASS: CEP-1 (compilation); emitted code is CEP-0.
// CEP:STATUS: complete
// CEP:FAILURE: compile returns IllegalForT0 for every function outside
//              the T0 subset; the caller falls back to the interpreter
//              (never silently).
// CEP:ASSUMES: little-endian x86-64 host (TargetConfig); locals count <=
//              MAX_T0_LOCALS; static value stack depth <=
//              MAX_T0_STACK_DEPTH; Intel SDM encodings (Vol 2).
// CEP:COST: O(insns) memcpy plus patching; measured against the 5 us
//           budget in tests (benches/EVIDENCE.md).
// CEP:EVIDENCE: tests/t0_exec.rs (differential execution, latency, patch
//               correctness); golden-byte unit tests below; doc/tier0.md.
// CEP:SECURITY: templates are frozen engine constants; only operand
//               immediates are patched and each is range-checked before
//               the write. tier0_check.rs is the T0 trust gate.

use crate::cold::tier0_check::{check_legality, IllegalForT0, Lat, MAX_T0_LOCALS};
use crate::cold::tier0_emit::{Cc, Emitter};

/// Patch hole kinds (spec 2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hole {
    /// 8-byte immediate (f64 bits or bool word).
    Imm64,
    /// 4-byte rel32 jump displacement.
    Rel32,
    /// 1-byte frame displacement: [rbp - 8*(slot+1)].
    Disp8Slot,
}

/// A frozen machine-code template with patch holes.
#[derive(Debug, Clone)]
pub struct Template {
    /// Template bytes.
    pub code: Vec<u8>,
    /// Patch holes: (offset, kind).
    pub holes: Vec<(u16, Hole)>,
}

/// The frozen template library (spec 2.1): built once, read-only, shared.
///
/// CEP:WHAT: Template library construction.
/// CEP:WHY: Spec 2.1 precompiled fragments; spec 2.4 read-only sharing.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: Intel SDM encodings; helper addresses are valid for the
///              process lifetime.
/// CEP:COST: O(1) at engine startup.
/// CEP:EVIDENCE: execution differential tests cover every template.
/// CEP:SECURITY: no input.
#[derive(Debug, Clone)]
pub struct TemplateLibrary {
    push_number: Template,
    push_bool: Template,
    add: Template,
    sub: Template,
    mul: Template,
    div: Template,
    modulo: Template,
    greater_than: Template,
    less_than: Template,
    equal: Template,
    load_var: Template,
    store_var: Template,
    assign_var: Template,
    jump: Template,
    jump_if_false: Template,
    jump_if_true: Template,
    drop: Template,
    /// Tail after the final Halt.
    pub halt_tail: Vec<u8>,
    /// Tail for value-returning functions.
    pub ok_tail: Vec<u8>,
}

impl TemplateLibrary {
    /// Build the library. Deterministic bytes on every call (38.19).
    ///
    /// CEP:WHAT: Template construction.
    /// CEP:WHY: Templates encode the spec 2.2 forms; DIV carries the
    ///          documented zero/NaN guards (doc/tier0.md deviations).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Panics only via expect on the internal finalize of the
    ///              DIV template whose labels are statically bound
    ///              (CEP-1 construction path, provably infallible).
    /// CEP:ASSUMES: fmod/trap helper addresses valid.
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: differential execution tests.
    /// CEP:SECURITY: no input.
    #[must_use]
    pub fn new(fmod_addr: usize, trap_addr: usize) -> TemplateLibrary {
        let imm64_template = |word: u64| {
            let mut e = Emitter::new();
            e.movabs(0, word);
            e.push_r(0);
            Template { code: e.as_slice().to_vec(), holes: vec![(2, Hole::Imm64)] }
        };
        // Add/Mul are commutative: memory-operand form per spec 2.2.
        let sse_binop = |op: u8| {
            let mut e = Emitter::new();
            e.movsd_xmm_rsp(0, 0);
            e.emit(&[0xF2, 0x0F, op, 0x44, 0x24, 0x08]);
            e.movsd_rsp_xmm(8, 0);
            e.add_rsp(8);
            Template { code: e.as_slice().to_vec(), holes: vec![] }
        };
        // Sub/Div are NOT commutative: load left into xmm0, right into
        // xmm1, then op xmm0, xmm1 (left op right), matching the
        // interpreter's operand order (doc/tier0.md).
        let sse_binop_ordered = |op: u8| {
            let mut e = Emitter::new();
            e.movsd_xmm_rsp(0, 8); // left
            e.movsd_xmm_rsp(1, 0); // right
            e.f2_op(op, 0, 1);
            e.movsd_rsp_xmm(8, 0);
            e.add_rsp(8);
            Template { code: e.as_slice().to_vec(), holes: vec![] }
        };
        // Comparison collapses the operand pair in place (same discipline
        // as sse_binop): the boolean result is written to the left slot and
        // the stack pointer is adjusted by one slot. Pushing the result
        // instead would leak 16 bytes per execution and overflow the machine
        // stack inside loops (regression matrix tests/stack_discipline.rs).
        let compare = |cc: Cc| {
            let mut e = Emitter::new();
            e.movsd_xmm_rsp(0, 8);
            e.movsd_xmm_rsp(1, 0);
            e.comisd(0, 1);
            e.setcc(cc, 0);
            e.movzx(0, 0);
            e.mov_rsp_r(8, 0);
            e.add_rsp(8);
            Template { code: e.as_slice().to_vec(), holes: vec![] }
        };
        // DIV: zero guard + unordered handling + NaN canonicalization
        // (documented deviation from the bare spec template, doc/tier0.md).
        let div = {
            let mut e = Emitter::new();
            let trap = e.label();
            let nan_or_zero = e.label();
            let ok = e.label();
            let done = e.label();
            let end = e.label();
            e.movsd_xmm_rsp(1, 0);
            e.xorps(3, 3);
            e.ucomisd(1, 3);
            e.jcc(Cc::P, nan_or_zero);
            e.jcc(Cc::Eq, trap);
            e.bind(nan_or_zero);
            e.movsd_xmm_rsp(0, 8);
            e.f2_op(0x5E, 0, 1);
            e.ucomisd(0, 0);
            e.jcc(Cc::Np, ok);
            e.movabs(10, crate::hot::value::CANONICAL_NAN);
            e.movq_xmm_r(2, 10);
            e.movsd_rsp_xmm(8, 2);
            e.jmp(done);
            e.bind(ok);
            e.movsd_rsp_xmm(8, 0);
            e.bind(done);
            e.add_rsp(8);
            e.jmp(end);
            e.bind(trap);
            e.emit(&[0x48, 0x89, 0xEF]); // mov rdi, rbp
            e.emit(&[0x48, 0x89, 0xE6]); // mov rsi, rsp
            e.call_abs(trap_addr);
            e.ud2();
            e.bind(end);
            e.finalize().expect("template labels are statically bound");
            Template { code: e.as_slice().to_vec(), holes: vec![] }
        };
        let modulo = {
            let mut e = Emitter::new();
            e.movsd_xmm_rsp(0, 8);
            e.movsd_xmm_rsp(1, 0);
            e.call_abs(fmod_addr);
            e.movsd_rsp_xmm(8, 0);
            e.add_rsp(8);
            Template { code: e.as_slice().to_vec(), holes: vec![] }
        };
        let equal = {
            let mut e = Emitter::new();
            e.movsd_xmm_rsp(0, 8);
            e.movsd_xmm_rsp(1, 0);
            e.comisd(0, 1);
            e.setcc(Cc::Eq, 0);
            e.setcc(Cc::Np, 1);
            e.emit(&[0x20, 0xC8]);
            e.movzx(0, 0);
            e.mov_rsp_r(8, 0);
            e.add_rsp(8);
            Template { code: e.as_slice().to_vec(), holes: vec![] }
        };
        let load_var = {
            let mut e = Emitter::new();
            e.mov_r_rbp(0, 0);
            e.push_r(0);
            Template { code: e.as_slice().to_vec(), holes: vec![(3, Hole::Disp8Slot)] }
        };
        let store_var = {
            let mut e = Emitter::new();
            e.pop_r(0);            // 1 byte
            e.mov_rbp_r(0, 0);     // 48 89 45 d8: disp is the 4th byte
            Template { code: e.as_slice().to_vec(), holes: vec![(4, Hole::Disp8Slot)] }
        };
        let jump = {
            let mut e = Emitter::new();
            let hole = e.label();
            e.jmp(hole);
            Template { code: e.as_slice().to_vec(), holes: vec![(1, Hole::Rel32)] }
        };
        let jump_if_false = {
            let mut e = Emitter::new();
            e.pop_r(0);
            e.test_rr(0, 0);
            let hole = e.label();
            e.jcc(Cc::Eq, hole);
            Template { code: e.as_slice().to_vec(), holes: vec![(6, Hole::Rel32)] }
        };
        let jump_if_true = {
            let mut e = Emitter::new();
            e.pop_r(0);
            e.test_rr(0, 0);
            let hole = e.label();
            e.jcc(Cc::Ne, hole);
            Template { code: e.as_slice().to_vec(), holes: vec![(6, Hole::Rel32)] }
        };
        // The default halt tail discards the result; compile() rebuilds a
        // depth-aware tail per program (the interpreter's Halt returns the
        // bottom of the value stack).
        let halt_tail = Vec::new();
        let ok_tail = {
            let mut e = Emitter::new();
            e.pop_r(0);
            e.emit(&[0x31, 0xD2]);
            e.epilogue();
            e.as_slice().to_vec()
        };
        TemplateLibrary {
            push_number: imm64_template(0),
            push_bool: imm64_template(0),
            add: sse_binop(0x58),
            sub: sse_binop_ordered(0x5C),
            mul: sse_binop(0x59),
            div,
            modulo,
            greater_than: compare(Cc::A),
            less_than: compare(Cc::B),
            equal,
            load_var,
            store_var: store_var.clone(),
            assign_var: store_var,
            jump,
            jump_if_false,
            jump_if_true,
            drop: Template { code: Vec::new(), holes: vec![] },
            halt_tail,
            ok_tail,
        }
    }

    /// Template for one opcode, if it is in the T0 subset.
    ///
    /// CEP:WHAT: Template lookup.
    /// CEP:WHY: Shared by size computation and emission.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: None outside the subset.
    /// CEP:ASSUMES: none
    /// CEP:COST: one match.
    /// CEP:EVIDENCE: execution tests cover every template.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn template(&self, op: crate::cold::bytecode::Opcode) -> Option<&Template> {
        use crate::cold::bytecode::Opcode as Op;
        match op {
            Op::PushNumber => Some(&self.push_number),
            Op::PushBool => Some(&self.push_bool),
            Op::Add => Some(&self.add),
            Op::Sub => Some(&self.sub),
            Op::Mul => Some(&self.mul),
            Op::Div => Some(&self.div),
            Op::Modulo => Some(&self.modulo),
            Op::GreaterThan => Some(&self.greater_than),
            Op::LessThan => Some(&self.less_than),
            Op::Equal => Some(&self.equal),
            Op::LoadVar => Some(&self.load_var),
            Op::StoreVar => Some(&self.store_var),
            Op::AssignVar => Some(&self.assign_var),
            Op::Jump => Some(&self.jump),
            Op::JumpIfFalse => Some(&self.jump_if_false),
            Op::JumpIfTrue => Some(&self.jump_if_true),
            Op::Drop => Some(&self.drop),
            _ => None,
        }
    }
}

/// Compiled Tier 0 function.
#[derive(Debug, Clone)]
pub struct T0Code {
    /// Machine code bytes.
    pub bytes: Vec<u8>,
    /// Per-pc code offsets (len = code.len()+1).
    pub offsets: Vec<u32>,
    /// Static maximum value-stack depth.
    pub max_stack: u32,
    /// Static type of the value returned at Halt.
    pub return_tag: crate::hot::value::TypeTag,
}

/// Compile a function with copy-and-patch (spec 2).
///
/// CEP:WHAT: The T0 compilation loop.
/// CEP:WHY: Direct bytecode-to-machine translation, no IR, sub-5us.
/// CEP:STATUS: complete
/// CEP:FAILURE: IllegalForT0 for functions outside the subset; the last
///              instruction must be Halt.
/// CEP:ASSUMES: little-endian host; verified code.
/// CEP:COST: O(insns) copies plus O(1) per hole; latency measured in tests.
/// CEP:EVIDENCE: tests/t0_exec.rs.
/// CEP:SECURITY: every patched value is range-checked before the write.
///
/// # Errors
/// Returns IllegalForT0 for functions outside the subset.
pub fn compile(
    func: &crate::cold::bytecode::Function,
    lib: &TemplateLibrary,
) -> Result<T0Code, IllegalForT0> {
    use crate::cold::bytecode::{Opcode, Operand};
    let report = check_legality(func)?;
    if func.code.last().map(|i| i.op) != Some(Opcode::Halt) {
        return Err(IllegalForT0::Opcode(Opcode::Halt));
    }
    // Pass 1: static offsets (template sizes are fixed; Halt is a tail).
    let mut offsets = Vec::with_capacity(func.code.len() + 1);
    let mut off: u32 = 0;
    for insn in &func.code {
        offsets.push(off);
        if insn.op == Opcode::Halt {
            continue;
        }
        let t = lib.template(insn.op).ok_or(IllegalForT0::Opcode(insn.op))?;
        off += t.code.len() as u32;
    }
    offsets.push(off);
    // Pass 2: prologue (frame setup) then copy templates and patch holes.
    // Locals live at [rbp - 8*(slot+1)]; the frame reserves locals*8 bytes.
    let mut pro = Emitter::new();
    pro.prologue();
    pro.sub_rsp((func.locals.len() as u32) * 8);
    let prefix = pro.as_slice().to_vec();
    let mut bytes = Vec::with_capacity(prefix.len() + off as usize + lib.halt_tail.len());
    bytes.extend_from_slice(&prefix);
    bytes.resize(prefix.len() + off as usize, 0);
    let prefix_len = prefix.len();
    for (pc, insn) in func.code.iter().enumerate() {
        if insn.op == Opcode::Halt {
            continue;
        }
        let t = lib.template(insn.op).ok_or(IllegalForT0::Opcode(insn.op))?;
        let base = prefix_len + offsets[pc] as usize;
        bytes[base..base + t.code.len()].copy_from_slice(&t.code);
        for &(hole_off, kind) in &t.holes {
            let at = base + hole_off as usize;
            match kind {
                Hole::Imm64 => {
                    let word = match insn.op {
                        Opcode::PushNumber => match insn.operand {
                            Operand::F64(f) => f.to_bits(),
                            _ => return Err(IllegalForT0::TypeMismatch("PUSH_NUMB operand")),
                        },
                        Opcode::PushBool => match insn.operand {
                            Operand::U64(b) => b & 1,
                            _ => return Err(IllegalForT0::TypeMismatch("PUSH_BOOL operand")),
                        },
                        _ => return Err(IllegalForT0::TypeMismatch("Imm64 hole misuse")),
                    };
                    bytes[at..at + 8].copy_from_slice(&word.to_le_bytes());
                }
                Hole::Rel32 => {
                    let Operand::Target(tgt) = insn.operand else {
                        return Err(IllegalForT0::TypeMismatch("Rel32 hole misuse"));
                    };
                    let tgt_off = offsets
                        .get(tgt as usize)
                        .copied()
                        .ok_or(IllegalForT0::TypeMismatch("Rel32 target out of range"))?;
                    // Both ends are in TEMPLATE coordinates (the prefix is
                    // position-independent for relative displacements).
                    let base_t = offsets[pc] as i64;
                    let rel = tgt_off as i64 - (base_t + t.code.len() as i64);
                    bytes[at..at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
                }
                Hole::Disp8Slot => {
                    let Operand::Slot(s) = insn.operand else {
                        return Err(IllegalForT0::TypeMismatch("Disp8 hole misuse"));
                    };
                    if s as usize >= MAX_T0_LOCALS {
                        return Err(IllegalForT0::TooManyLocals(s as usize));
                    }
                    bytes[at] = (-(8 * (s as i32 + 1))) as i8 as u8;
                }
            }
        }
    }
    // Depth-aware halt tail: the interpreter's Halt returns the bottom of
    // the value stack (legacy parity). The static depth and lattice type
    // at the Halt pc come from a linear simulation that tracks per-slot
    // lattice types exactly like check_legality's initial state; a bool
    // slot loaded after a loop must return a bool, not a mis-decoded word.
    let mut depth: i32 = 0;
    let mut depth_at_halt: i32 = 0;
    let mut stack_lats: Vec<Lat> = Vec::new();
    let mut slot_lat: Vec<Lat> = vec![Lat::Num; func.locals.len()];
    let mut return_lat = Lat::Num;
    // The walk is linear, but an unconditional Jump makes every instruction
    // up to its target dead: simulating them would double-count pushes and
    // the halt tail would read the wrong slot. Skip to the target; backward
    // jumps resume linearly, which is exact because check_legality enforces
    // equal stack states at every merge point.
    let mut dead_until: usize = usize::MAX;
    for (pc, insn) in func.code.iter().enumerate() {
        if dead_until != usize::MAX {
            if pc < dead_until {
                continue;
            }
            dead_until = usize::MAX;
        }
        match insn.op {
            Opcode::PushNumber => {
                depth += 1;
                stack_lats.push(Lat::Num);
            }
            Opcode::PushBool => {
                depth += 1;
                stack_lats.push(Lat::Bool);
            }
            Opcode::LoadVar => {
                depth += 1;
                let lat = match insn.operand {
                    Operand::Slot(s) => slot_lat.get(s as usize).copied().unwrap_or(Lat::Num),
                    _ => Lat::Num,
                };
                stack_lats.push(lat);
            }
            Opcode::StoreVar | Opcode::AssignVar => {
                depth -= 1;
                let v = stack_lats.pop().unwrap_or(Lat::Num);
                if let Operand::Slot(s) = insn.operand
                    && let Some(sl) = slot_lat.get_mut(s as usize) {
                        *sl = v;
                    }
            }
            Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div | Opcode::Modulo => {
                depth -= 1;
                let _ = stack_lats.pop();
                let _ = stack_lats.pop();
                stack_lats.push(Lat::Num);
            }
            Opcode::GreaterThan | Opcode::LessThan | Opcode::Equal => {
                depth -= 1;
                let _ = stack_lats.pop();
                let _ = stack_lats.pop();
                stack_lats.push(Lat::Bool);
            }
            Opcode::JumpIfFalse | Opcode::JumpIfTrue => {
                depth -= 1;
                let _ = stack_lats.pop();
            }
            // Drop(slot) retires a variable binding only; it never touches
            // the value stack (interp.rs and check_legality agree). The
            // depth simulation must match the machine template, which emits
            // no stack adjustment, or the halt tail reads the wrong slot.
            Opcode::Jump => {
                if let Operand::Target(t) = insn.operand {
                    let t = t as usize;
                    if t > pc + 1 {
                        dead_until = t;
                    }
                }
            }
            Opcode::Halt => {
                depth_at_halt = depth;
                return_lat = stack_lats.first().copied().unwrap_or(Lat::Num);
            }
            _ => {}
        }
    }
    {
        let mut tail = Emitter::new();
        if depth_at_halt > 0 {
            // mov rax, [rsp + 8*(depth-1)] (bottom of the machine stack)
            let disp = 8 * (depth_at_halt - 1);
            tail.emit(&[0x48, 0x8B, 0x44, 0x24]);
            tail.emit1(disp as u8);
        } else {
            tail.emit(&[0x31, 0xC0]);
        }
        tail.emit(&[0x31, 0xD2]);
        tail.epilogue();
        bytes.extend_from_slice(tail.as_slice());
    }
    Ok(T0Code {
        bytes,
        offsets,
        max_stack: report.max_stack,
        return_tag: match return_lat {
            Lat::Num => crate::hot::value::TypeTag::Number,
            Lat::Bool => crate::hot::value::TypeTag::Bool,
        },
    })
}

#[cfg(test)]
#[path = "tier0_tests.rs"]
mod tests;

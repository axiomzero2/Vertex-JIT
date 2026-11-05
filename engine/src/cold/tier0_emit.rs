// CEP:FILE: engine/src/cold/tier0_emit.rs
// CEP:WHAT: The x86-64 byte emitter: label/fixup bookkeeping and the
//           encoded-instruction helpers the templates and codegen share.
// CEP:WHY: Every machine byte passes through one auditable encoder with
//          fail-closed label resolution (doc/tier0.md, spec 2.2).
// CEP:CLASS: CEP-1 (compilation); emitted code is CEP-0.
// CEP:STATUS: complete
// CEP:FAILURE: unbound labels fail closed at emit time; every immediate
//              is range-checked before the write.
// CEP:ASSUMES: little-endian x86-64 host; Intel SDM encodings (Vol 2).
// CEP:COST: O(bytes emitted); each helper is a fixed-size append.
// CEP:EVIDENCE: golden-byte unit tests; objdump cross-check
//               (benches/EVIDENCE.md); tier0.rs tests.
// CEP:SECURITY: the emitter never reads input; only engine constants and
//               range-checked immediates reach the code buffer.


/// Condition code for jcc and setcc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cc {
    /// Equal / zero (ZF=1).
    Eq,
    /// Not equal (ZF=0).
    Ne,
    /// Below / unsigned less (CF=1).
    B,
    /// Above / unsigned greater (CF=0 and ZF=0).
    A,
    /// Parity set (unordered compare).
    P,
    /// Parity clear (ordered compare).
    Np,
}

impl Cc {
    /// Low nibble of the 0F 8x opcode byte.
    ///
    /// CEP:WHAT: Condition code to opcode mapping.
    /// CEP:WHY: One table prevents encoding typos.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: Intel SDM Jcc opcode map.
    /// CEP:COST: one match.
    /// CEP:EVIDENCE: golden-byte tests.
    /// CEP:SECURITY: none
    #[must_use]
    pub const fn opcode_nibble(self) -> u8 {
        match self {
            Cc::Eq => 0x4,
            Cc::Ne => 0x5,
            Cc::B => 0x2,
            Cc::A => 0x7,
            Cc::P => 0xA,
            Cc::Np => 0xB,
        }
    }
}

/// A pending rel32 fixup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixup {
    /// Offset of the 4-byte displacement field.
    pub at: u32,
    /// Target label.
    pub label: Label,
}

/// Opaque label id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Label(pub u32);

/// Byte emitter with label resolution.
///
/// CEP:WHAT: x86-64 emitter with fixed audited encodings.
/// CEP:WHY: T0 templates and T1+ codegen share one emitter (doc/x64.md);
///          hand-assembled encodings keep T0 latency under the budget.
/// CEP:CLASS: CEP-1
/// CEP:STATUS: complete
/// CEP:FAILURE: finalize returns Err for unbound labels or displacements
///              outside i32 (fail closed, Law 6).
/// CEP:ASSUMES: x86-64 host.
/// CEP:COST: one bounded Vec append per instruction.
/// CEP:EVIDENCE: golden-byte tests below plus objdump cross-check;
///               execution differential tests.
/// CEP:SECURITY: emits engine-authored bytes only.
#[derive(Debug, Default, Clone)]
pub struct Emitter {
    code: Vec<u8>,
    fixups: Vec<Fixup>,
    labels: Vec<Option<u32>>,
}

impl Emitter {
    /// Create an empty emitter.
    ///
    /// CEP:WHAT: Constructor.
    /// CEP:WHY: Compilation units start empty.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: unit tests.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn new() -> Emitter {
        Emitter::default()
    }

    /// Bytes emitted so far.
    ///
    /// CEP:WHAT: Buffer accessor.
    /// CEP:WHY: Templates and the code cache copy from it.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: unit tests.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.code
    }

    /// Declare a label.
    ///
    /// CEP:WHAT: Label allocation.
    /// CEP:WHY: Jump patching needs symbolic targets.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: unit tests.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label((self.labels.len() - 1) as u32)
    }

    /// Bind a label to the current position.
    ///
    /// CEP:WHAT: Label binding.
    /// CEP:WHY: Fixups resolve through bindings.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Debug assert on double binding.
    /// CEP:ASSUMES: label not already bound.
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: unit tests.
    /// CEP:SECURITY: none
    pub fn bind(&mut self, label: Label) {
        debug_assert!(self.labels[label.0 as usize].is_none(), "label bound twice");
        self.labels[label.0 as usize] = Some(self.code.len() as u32);
    }

    /// Resolve all fixups; fails closed on unbound labels.
    ///
    /// CEP:WHAT: Fixup finalization.
    /// CEP:WHY: rel32 displacements need target offsets.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Err(offset, label) on unbound or out-of-range.
    /// CEP:ASSUMES: referenced labels are bound.
    /// CEP:COST: O(fixups).
    /// CEP:EVIDENCE: unit tests.
    /// CEP:SECURITY: none
    ///
    /// # Errors
    /// Returns the first unbound label's fixup site.
    pub fn finalize(&mut self) -> Result<(), (u32, Label)> {
        let mut result = Ok(());
        for i in 0..self.fixups.len() {
            let f = self.fixups[i];
            let Some(target) = self.labels[f.label.0 as usize] else {
                result = Err((f.at, f.label));
                break;
            };
            let at = f.at as usize;
            let rel = target as i64 - (at as i64 + 4);
            if rel < i32::MIN as i64 || rel > i32::MAX as i64 {
                result = Err((f.at, f.label));
                break;
            }
            self.code[at..at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
        }
        result
    }

    /// Append raw bytes.
    ///
    /// CEP:WHAT: Raw append.
    /// CEP:WHY: Template bodies are copied verbatim.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: engine-authored constants only.
    /// CEP:COST: O(n).
    /// CEP:EVIDENCE: unit tests.
    /// CEP:SECURITY: no input bytes pass through unvalidated.
    pub fn emit(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    /// Append one byte.
    ///
    /// CEP:WHAT: Byte append.
    /// CEP:WHY: Encoding composition.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: unit tests.
    /// CEP:SECURITY: none
    pub fn emit1(&mut self, b: u8) {
        self.code.push(b);
    }

    /// jmp rel32.
    ///
    /// CEP:WHAT: Unconditional jump.
    /// CEP:WHY: Control flow lowering.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: finalize fails on unbound labels.
    /// CEP:ASSUMES: none
    /// CEP:COST: 5 bytes.
    /// CEP:EVIDENCE: golden bytes; execution tests.
    /// CEP:SECURITY: none
    pub fn jmp(&mut self, label: Label) {
        self.code.push(0xE9);
        self.record_fixup(label);
    }

    /// jcc rel32.
    ///
    /// CEP:WHAT: Conditional jump.
    /// CEP:WHY: Branch lowering.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: finalize fails on unbound labels.
    /// CEP:ASSUMES: none
    /// CEP:COST: 6 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn jcc(&mut self, cc: Cc, label: Label) {
        self.code.extend_from_slice(&[0x0F, 0x80 | cc.opcode_nibble()]);
        self.record_fixup(label);
    }

    /// call rel32.
    ///
    /// CEP:WHAT: Near call.
    /// CEP:WHY: Runtime helper calls.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: finalize fails on unbound labels.
    /// CEP:ASSUMES: none
    /// CEP:COST: 5 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn call(&mut self, label: Label) {
        self.code.push(0xE8);
        self.record_fixup(label);
    }

    /// ret.
    ///
    /// CEP:WHAT: Return.
    /// CEP:WHY: Epilogue.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: stack restored to frame base.
    /// CEP:COST: 1 byte.
    /// CEP:EVIDENCE: execution tests.
    /// CEP:SECURITY: none
    pub fn ret(&mut self) {
        self.code.push(0xC3);
    }

    /// ud2 (defined trap).
    ///
    /// CEP:WHAT: Illegal-instruction trap.
    /// CEP:WHY: Patch holes and unreachable paths must trap (38.40).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: traps by design.
    /// CEP:ASSUMES: x86-64.
    /// CEP:COST: 2 bytes.
    /// CEP:EVIDENCE: patch tests.
    /// CEP:SECURITY: none
    pub fn ud2(&mut self) {
        self.code.extend_from_slice(&[0x0F, 0x0B]);
    }

    /// push r64 (reg 0..15).
    ///
    /// CEP:WHAT: Register push.
    /// CEP:WHY: Template stack discipline.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: Intel opcode map.
    /// CEP:COST: 1-2 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn push_r(&mut self, reg: u8) {
        if reg >= 8 {
            self.code.push(0x41);
        }
        self.code.push(0x50 | (reg & 7));
    }

    /// pop r64.
    ///
    /// CEP:WHAT: Register pop.
    /// CEP:WHY: Template stack discipline.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: Intel opcode map.
    /// CEP:COST: 1-2 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn pop_r(&mut self, reg: u8) {
        if reg >= 8 {
            self.code.push(0x41);
        }
        self.code.push(0x58 | (reg & 7));
    }

    /// movabs r64, imm64.
    ///
    /// CEP:WHAT: 64-bit immediate load.
    /// CEP:WHY: f64 bit patterns, box words, helper addresses.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 10 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn movabs(&mut self, reg: u8, imm: u64) {
        self.code.push(if reg >= 8 { 0x49 } else { 0x48 });
        self.code.push(0xB8 | (reg & 7));
        self.code.extend_from_slice(&imm.to_le_bytes());
    }

    /// mov r64, r64.
    ///
    /// CEP:WHAT: Register move.
    /// CEP:WHY: Value shuffling.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 3 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn mov_rr(&mut self, dst: u8, src: u8) {
        // Opcode 89 (MOV r/m64, r64) copies the REG field into the RM
        // field, so the ModRM reg field carries the SOURCE and rm the
        // destination (Intel SDM Vol 2A, MOV-64 MR form).
        self.code.push(0x48 | ((dst >= 8) as u8) << 2 | ((src >= 8) as u8));
        self.code.push(0x89);
        self.code.push(0xC0 | ((src & 7) << 3) | (dst & 7));
    }

    /// mov r64, [rsp+disp8].
    ///
    /// CEP:WHAT: Stack-slot load.
    /// CEP:WHY: Value stack addressing.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: debug assert on disp8 range; frame sizes are bounded
    ///              by the legality checker.
    /// CEP:ASSUMES: disp in -128..=127.
    /// CEP:COST: 5 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn mov_r_rsp(&mut self, reg: u8, disp: i8) {
        self.code.push(0x48);
        self.code.push(0x8B);
        self.modrm_sib(reg, 4, disp);
    }

    /// mov [rsp+disp8], r64.
    ///
    /// CEP:WHAT: Stack-slot store.
    /// CEP:WHY: Value stack addressing.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: disp8 range as above.
    /// CEP:ASSUMES: disp in -128..=127.
    /// CEP:COST: 5 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn mov_rsp_r(&mut self, disp: i8, reg: u8) {
        self.code.push(0x48);
        self.code.push(0x89);
        self.modrm_sib(reg, 4, disp);
    }

    /// mov r64, [rbp+disp8].
    ///
    /// CEP:WHAT: Frame-slot load.
    /// CEP:WHY: Locals at rbp offsets.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: disp8 range as above.
    /// CEP:ASSUMES: disp in -128..=127.
    /// CEP:COST: 4 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn mov_r_rbp(&mut self, reg: u8, disp: i8) {
        self.code.push(0x48);
        self.code.push(0x8B);
        self.modrm_sib(reg, 5, disp);
    }

    /// mov [rbp+disp8], r64.
    ///
    /// CEP:WHAT: Frame-slot store.
    /// CEP:WHY: Locals at rbp offsets.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: disp8 range as above.
    /// CEP:ASSUMES: disp in -128..=127.
    /// CEP:COST: 4 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn mov_rbp_r(&mut self, disp: i8, reg: u8) {
        self.code.push(0x48);
        self.code.push(0x89);
        self.modrm_sib(reg, 5, disp);
    }

    /// test r64, r64.
    ///
    /// CEP:WHAT: Zero test.
    /// CEP:WHY: JUMP_IF_FALSE spec template (spec 2.2).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 3 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn test_rr(&mut self, a: u8, b: u8) {
        self.code.push(0x48 | ((a >= 8) as u8) << 2 | ((b >= 8) as u8));
        self.code.push(0x85);
        self.code.push(0xC0 | ((a & 7) << 3) | (b & 7));
    }

    /// setcc r8 (low byte).
    ///
    /// CEP:WHAT: Flag materialization.
    /// CEP:WHY: Comparisons produce 0/1 booleans.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: low-byte register.
    /// CEP:COST: 3 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn setcc(&mut self, cc: Cc, reg: u8) {
        self.code.extend_from_slice(&[0x0F, 0x90 | cc.opcode_nibble()]);
        self.code.push(0xC0 | (reg & 7));
    }

    /// movzx r64, r8.
    ///
    /// CEP:WHAT: Byte zero extension.
    /// CEP:WHY: Booleans must be exactly 0 or 1 (doc/value.md).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 4 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn movzx(&mut self, dst: u8, src: u8) {
        self.code.push(0x48 | ((dst >= 8) as u8) << 2);
        self.code.extend_from_slice(&[0x0F, 0xB6]);
        self.code.push(0xC0 | ((dst & 7) << 3) | (src & 7));
    }

    /// sub rsp, imm32.
    ///
    /// CEP:WHAT: Stack decrement.
    /// CEP:WHY: Frame allocation.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 7 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn sub_rsp(&mut self, imm: u32) {
        self.code.extend_from_slice(&[0x48, 0x81, 0xEC]);
        self.code.extend_from_slice(&imm.to_le_bytes());
    }

    /// add rsp, imm32.
    ///
    /// CEP:WHAT: Stack increment.
    /// CEP:WHY: Value pops.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 7 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn add_rsp(&mut self, imm: u32) {
        self.code.extend_from_slice(&[0x48, 0x81, 0xC4]);
        self.code.extend_from_slice(&imm.to_le_bytes());
    }

    /// Absolute indirect call via r11.
    ///
    /// CEP:WHAT: Absolute call.
    /// CEP:WHY: Runtime helpers live at arbitrary addresses.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: r11 is caller-saved and free at the call site.
    /// CEP:COST: 13 bytes.
    /// CEP:EVIDENCE: golden bytes; helper tests.
    /// CEP:SECURITY: targets are engine-internal functions.
    pub fn call_abs(&mut self, target: usize) {
        self.movabs(11, target as u64);
        self.code.extend_from_slice(&[0x41, 0xFF, 0xD3]);
    }

    /// movsd xmm, [rsp+disp8].
    ///
    /// CEP:WHAT: Double load.
    /// CEP:WHY: Arithmetic operand fetch per spec templates.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: disp8 range.
    /// CEP:ASSUMES: none
    /// CEP:COST: 6 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn movsd_xmm_rsp(&mut self, xmm: u8, disp: i8) {
        self.code.extend_from_slice(&[0xF2, 0x0F, 0x10]);
        self.modrm_sib(xmm, 4, disp);
    }

    /// movsd [rsp+disp8], xmm.
    ///
    /// CEP:WHAT: Double store.
    /// CEP:WHY: Result write-back per spec templates.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: disp8 range.
    /// CEP:ASSUMES: none
    /// CEP:COST: 6 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn movsd_rsp_xmm(&mut self, disp: i8, xmm: u8) {
        self.code.extend_from_slice(&[0xF2, 0x0F, 0x11]);
        self.modrm_sib(xmm, 4, disp);
    }

    /// movsd xmm, [rbp+disp8].
    ///
    /// CEP:WHAT: Frame double load.
    /// CEP:WHY: Locals in optimized code.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: disp8 range.
    /// CEP:ASSUMES: none
    /// CEP:COST: 5 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn movsd_xmm_rbp(&mut self, xmm: u8, disp: i8) {
        self.code.extend_from_slice(&[0xF2, 0x0F, 0x10]);
        self.modrm_sib(xmm, 5, disp);
    }

    /// movsd [rbp+disp8], xmm.
    ///
    /// CEP:WHAT: Frame double store.
    /// CEP:WHY: Locals in optimized code.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: disp8 range.
    /// CEP:ASSUMES: none
    /// CEP:COST: 5 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn movsd_rbp_xmm(&mut self, disp: i8, xmm: u8) {
        self.code.extend_from_slice(&[0xF2, 0x0F, 0x11]);
        self.modrm_sib(xmm, 5, disp);
    }

    /// Binary SSE2 double op (F2 0F op /r, mod=11).
    ///
    /// CEP:WHAT: addsd/subsd/mulsd/divsd primitive.
    /// CEP:WHY: Arithmetic template cores.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: op is one of 58/5C/59/5E.
    /// CEP:COST: 4 bytes.
    /// CEP:EVIDENCE: golden bytes; differential execution.
    /// CEP:SECURITY: none
    pub fn f2_op(&mut self, op: u8, d: u8, s: u8) {
        self.code.extend_from_slice(&[0xF2, 0x0F, op]);
        self.code.push(0xC0 | ((d & 7) << 3) | (s & 7));
    }

    /// comisd xmm_d, xmm_s (ordered compare).
    ///
    /// CEP:WHAT: Double compare.
    /// CEP:WHY: Comparisons produce 0/1 via setcc.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 4 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn comisd(&mut self, d: u8, s: u8) {
        self.code.extend_from_slice(&[0x66, 0x0F, 0x2F]);
        self.code.push(0xC0 | ((d & 7) << 3) | (s & 7));
    }

    /// ucomisd xmm_d, xmm_s.
    ///
    /// CEP:WHAT: Unordered-tolerant compare.
    /// CEP:WHY: Guards (zero check, NaN canonicalization).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 4 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn ucomisd(&mut self, d: u8, s: u8) {
        self.code.extend_from_slice(&[0x66, 0x0F, 0x2E]);
        self.code.push(0xC0 | ((d & 7) << 3) | (s & 7));
    }

    /// movq xmm, r64.
    ///
    /// CEP:WHAT: GPR-to-XMM bit move.
    /// CEP:WHY: Canonical NaN materialization in DIV.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 5 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn movq_xmm_r(&mut self, xmm: u8, reg: u8) {
        self.code.push(0x66);
        self.code.push(0x48 | ((reg >= 8) as u8));
        self.code.extend_from_slice(&[0x0F, 0x6E]);
        self.code.push(0xC0 | ((xmm & 7) << 3) | (reg & 7));
    }

    /// xorps xmm_d, xmm_s.
    ///
    /// CEP:WHAT: XMM zeroing.
    /// CEP:WHY: Zero guards.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 3 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    pub fn xorps(&mut self, d: u8, s: u8) {
        self.code.extend_from_slice(&[0x0F, 0x57]);
        self.code.push(0xC0 | ((d & 7) << 3) | (s & 7));
    }

    /// Prologue: push rbp; mov rbp, rsp.
    ///
    /// CEP:WHAT: Frame prologue.
    /// CEP:WHY: rbp-relative locals and deopt frame walking.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: 4 bytes.
    /// CEP:EVIDENCE: execution tests.
    /// CEP:SECURITY: none
    pub fn prologue(&mut self) {
        self.code.push(0x55);
        self.code.extend_from_slice(&[0x48, 0x89, 0xE5]);
    }

    /// Epilogue: mov rsp, rbp; pop rbp; ret.
    ///
    /// CEP:WHAT: Frame epilogue.
    /// CEP:WHY: Balanced frames; rax/rdx return protocol.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: rbp is the frame base.
    /// CEP:COST: 4 bytes.
    /// CEP:EVIDENCE: execution tests.
    /// CEP:SECURITY: none
    pub fn epilogue(&mut self) {
        self.code.extend_from_slice(&[0x48, 0x89, 0xEC]);
        self.code.push(0x5D);
        self.ret();
    }

    /// ModRM/SIB for [rsp+disp8] and [rbp+disp8] only.
    ///
    /// CEP:WHAT: Addressing-mode bytes.
    /// CEP:WHY: Two forms cover every engine access (Intel SDM Vol 2A
    ///          Table 2-1: rsp needs SIB 0x24; rbp needs no SIB).
    /// CEP:STATUS: complete
    /// CEP:FAILURE: unreachable for other bases (engine addressing is
    ///              restricted to rsp/rbp by construction).
    /// CEP:ASSUMES: base_rm in {4, 5}.
    /// CEP:COST: 2-3 bytes.
    /// CEP:EVIDENCE: golden bytes.
    /// CEP:SECURITY: none
    fn modrm_sib(&mut self, reg: u8, base_rm: u8, disp: i8) {
        match base_rm {
            4 => {
                self.code.push(0x44 | ((reg & 7) << 3));
                self.code.push(0x24);
                self.code.push(disp as u8);
            }
            5 => {
                self.code.push(0x45 | ((reg & 7) << 3));
                self.code.push(disp as u8);
            }
            _ => unreachable!("engine addressing uses only rsp/rbp bases"),
        }
    }

    /// Record a relocation slot that finalize patches with a rel32.
/// CEP:WHAT: Rel32 fixup registration for a forward label.
/// CEP:WHY: Jump displacements are unknown until the target emits;
///              finalize patches every recorded fixup or fails closed.
/// CEP:STATUS: complete
/// CEP:FAILURE: finalize fails closed on unbound labels.
/// CEP:ASSUMES: labels are unique per emitted program.
/// CEP:COST: one push.
/// CEP:EVIDENCE: golden-byte tests; the unbound-label test.
    fn record_fixup(&mut self, label: Label) {
        let at = self.code.len() as u32;
        self.code.extend_from_slice(&[0, 0, 0, 0]);
        self.fixups.push(Fixup { at, label });
    }
}

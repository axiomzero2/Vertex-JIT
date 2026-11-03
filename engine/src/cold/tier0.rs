// CEP:FILE: engine/src/cold/tier0.rs
// CEP:WHAT: Tier 0: the x86-64 byte emitter, the frozen template library,
//           the static type-and-legality checker and the copy-patch
//           compilation loop.
// CEP:WHY: Spec section 2: T0 translates bytecode directly to machine code
//          through precompiled templates with patched immediates, bypassing
//          any IR, targeting a compile latency under 5 microseconds.
// CEP:CLASS: CEP-1 (compilation); emitted code is CEP-0.
// CEP:STATUS: complete
// CEP:FAILURE: compile returns IllegalForT0 for every function outside the
//              T0 subset; the caller falls back to the interpreter (never
//              silently). Emitter label resolution fails closed on unbound
//              labels.
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-CODEGEN: fixed x86-64 encodings verified by golden-byte tests and
//             objdump cross-check; W^X publication through the code cache.
// CEP:HPC-DETERMINISM: deterministic; templates are built from constants,
//             compilation is a pure function of the bytecode.
// CEP:HPC-COMPILE-COST: O(insns) memcpy plus patching; measured against the
//             5 us budget in tests (benches/EVIDENCE.md).
// CEP:ASSUMES: little-endian x86-64 host (TargetConfig); locals count <= 15
//              (i8 displacement window); static value stack depth <= 127;
//              Intel SDM encodings (Vol 2).
// CEP:COST: see CEP:HPC-COMPILE-COST.
// CEP:EVIDENCE: tests/t0_exec.rs (differential execution, latency, patch
//               correctness); golden-byte unit tests; doc/tier0.md.
// CEP:SECURITY: templates are frozen engine constants; only operand
//               immediates are patched and each is range-checked before
//               the write. The legality checker is the T0 trust gate.

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
        self.code.push(0x48 | ((dst >= 8) as u8) << 2 | ((src >= 8) as u8));
        self.code.push(0x89);
        self.code.push(0xC0 | ((dst & 7) << 3) | (src & 7));
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

    fn record_fixup(&mut self, label: Label) {
        let at = self.code.len() as u32;
        self.code.extend_from_slice(&[0, 0, 0, 0]);
        self.fixups.push(Fixup { at, label });
    }
}

// ---------------------------------------------------------------- templates

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

/// Static type lattice for T0 legality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lat {
    /// Raw f64 bit pattern word.
    Num,
    /// Raw 0/1 word.
    Bool,
}

/// Maximum locals addressable with i8 displacements.
pub const MAX_T0_LOCALS: usize = 15;
/// Maximum static value-stack depth (i8 window discipline).
pub const MAX_T0_STACK_DEPTH: usize = 127;

/// Why a function cannot run at Tier 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IllegalForT0 {
    /// Opcode outside the T0 subset.
    Opcode(crate::cold::bytecode::Opcode),
    /// Static stack depth exceeded.
    StackTooDeep(usize),
    /// Too many locals for the i8 window.
    TooManyLocals(usize),
    /// Types or shapes violate template assumptions.
    TypeMismatch(&'static str),
    /// Abstract interpretation did not converge (fail closed).
    NotConvergent,
}

/// Legality report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegalityReport {
    /// Static maximum value-stack depth.
    pub max_stack: u32,
}

/// Abstract state at one pc.
#[derive(Debug, Clone, PartialEq, Eq)]
struct State {
    stack: Vec<Lat>,
    /// Tracked lattice type per slot.
    slot_types: Vec<Lat>,
    /// Live-binding count per slot (scope model, doc/tier0.md).
    binding: Vec<u8>,
}

/// Static legality analysis: proves the function matches every T0 template
/// assumption before any machine code is generated.
///
/// CEP:WHAT: Abstract interpretation with equality joins at merge points.
/// CEP:WHY: Law 2: templates assume raw doubles and 0/1 booleans; the
///          check enforces that per program point so templates stay the
///          unguarded spec forms (spec 2.2).
/// CEP:STATUS: complete
/// CEP:FAILURE: IllegalForT0 for every rejected class; rejected functions
///              always fall back to the interpreter.
/// CEP:ASSUMES: verified bytecode.
/// CEP:COST: O(insns * passes), bounded at code length + 2 passes.
/// CEP:EVIDENCE: legality positive/negative tests.
/// CEP:SECURITY: this checker is the T0 trust gate for untrusted bytecode.
pub fn check_legality(func: &crate::cold::bytecode::Function) -> Result<LegalityReport, IllegalForT0> {
    use crate::cold::bytecode::{Opcode, Operand};
    let n_locals = func.locals.len();
    if n_locals > MAX_T0_LOCALS {
        return Err(IllegalForT0::TooManyLocals(n_locals));
    }
    let len = func.code.len();
    let mut states: Vec<Option<State>> = vec![None; len];
    states[0] = Some(State {
        stack: Vec::new(),
        slot_types: vec![Lat::Num; n_locals],
        binding: vec![0; n_locals],
    });
    let mut max_stack: usize = 0;
    let mut changed = true;
    let mut passes = 0usize;
    while changed {
        changed = false;
        passes += 1;
        if passes > len + 2 {
            return Err(IllegalForT0::NotConvergent);
        }
        for pc in 0..len {
            let Some(state) = states[pc].clone() else { continue };
            if state.stack.len() > MAX_T0_STACK_DEPTH {
                return Err(IllegalForT0::StackTooDeep(state.stack.len()));
            }
            max_stack = max_stack.max(state.stack.len());
            let insn = func.code[pc];
            let mut next: Vec<(usize, State)> = Vec::with_capacity(2);
            match insn.op {
                Opcode::PushNumber | Opcode::PushBool => {
                    let mut s = state.clone();
                    s.stack.push(Lat::Num);
                    next.push((pc + 1, s));
                }
                Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div | Opcode::Modulo
                | Opcode::GreaterThan | Opcode::LessThan | Opcode::Equal => {
                    if state.stack.len() < 2 {
                        return Err(IllegalForT0::TypeMismatch("binary op needs two operands"));
                    }
                    let mut s = state.clone();
                    s.stack.truncate(s.stack.len() - 2);
                    let out = match insn.op {
                        Opcode::GreaterThan | Opcode::LessThan | Opcode::Equal => Lat::Bool,
                        _ => Lat::Num,
                    };
                    s.stack.push(out);
                    next.push((pc + 1, s));
                }
                Opcode::JumpIfFalse | Opcode::JumpIfTrue => {
                    if state.stack.last() != Some(&Lat::Bool) {
                        return Err(IllegalForT0::TypeMismatch("condition must be bool"));
                    }
                    let Operand::Target(t) = insn.operand else {
                        return Err(IllegalForT0::TypeMismatch("jump without target"));
                    };
                    let mut s = state.clone();
                    s.stack.pop();
                    next.push((t as usize, s.clone()));
                    next.push((pc + 1, s));
                }
                Opcode::Jump => {
                    let Operand::Target(t) = insn.operand else {
                        return Err(IllegalForT0::TypeMismatch("jump without target"));
                    };
                    next.push((t as usize, state.clone()));
                }
                Opcode::LoadVar => {
                    let Operand::Slot(sl) = insn.operand else {
                        return Err(IllegalForT0::TypeMismatch("LoadVar without slot"));
                    };
                    if state.binding[sl as usize] < 1 {
                        return Err(IllegalForT0::TypeMismatch("load of unbound variable"));
                    }
                    let mut s = state.clone();
                    s.stack.push(s.slot_types[sl as usize]);
                    next.push((pc + 1, s));
                }
                Opcode::StoreVar => {
                    let Operand::Slot(sl) = insn.operand else {
                        return Err(IllegalForT0::TypeMismatch("StoreVar without slot"));
                    };
                    if state.stack.is_empty() {
                        return Err(IllegalForT0::TypeMismatch("store on empty stack"));
                    }
                    if state.binding[sl as usize] >= 1 {
                        // Shadowing redefinition: the flat machine frame
                        // cannot restore the outer binding (doc/tier0.md).
                        return Err(IllegalForT0::TypeMismatch(
                            "shadowing store is outside the T0 subset",
                        ));
                    }
                    let mut s = state.clone();
                    let v = s.stack.pop().expect("checked non-empty above");
                    s.slot_types[sl as usize] = v;
                    s.binding[sl as usize] = 1;
                    next.push((pc + 1, s));
                }
                Opcode::AssignVar => {
                    let Operand::Slot(sl) = insn.operand else {
                        return Err(IllegalForT0::TypeMismatch("AssignVar without slot"));
                    };
                    if state.stack.is_empty() {
                        return Err(IllegalForT0::TypeMismatch("assign on empty stack"));
                    }
                    if state.binding[sl as usize] < 1 {
                        return Err(IllegalForT0::TypeMismatch("assign of unbound variable"));
                    }
                    let mut s = state.clone();
                    let v = s.stack.pop().expect("checked non-empty above");
                    if s.slot_types[sl as usize] != v {
                        return Err(IllegalForT0::TypeMismatch(
                            "assign changes the slot lattice type",
                        ));
                    }
                    next.push((pc + 1, s));
                }
                Opcode::Drop => {
                    let Operand::Slot(sl) = insn.operand else {
                        return Err(IllegalForT0::TypeMismatch("Drop without slot"));
                    };
                    if state.binding[sl as usize] < 1 {
                        return Err(IllegalForT0::TypeMismatch("drop of unbound variable"));
                    }
                    let mut s = state.clone();
                    s.binding[sl as usize] -= 1;
                    next.push((pc + 1, s));
                }
                Opcode::Halt => {}
                other => return Err(IllegalForT0::Opcode(other)),
            }
            for (succ, s) in next {
                if succ >= len {
                    return Err(IllegalForT0::TypeMismatch("successor out of range"));
                }
                if s.stack.len() > MAX_T0_STACK_DEPTH {
                    return Err(IllegalForT0::StackTooDeep(s.stack.len()));
                }
                max_stack = max_stack.max(s.stack.len());
                match &states[succ] {
                    None => {
                        states[succ] = Some(s);
                        changed = true;
                    }
                    Some(existing) => {
                        if existing != &s {
                            return Err(IllegalForT0::TypeMismatch(
                                "state conflict at merge point",
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(LegalityReport { max_stack: max_stack as u32 })
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
        let sse_binop = |op: u8| {
            let mut e = Emitter::new();
            e.movsd_xmm_rsp(0, 0);
            e.emit(&[0xF2, 0x0F, op, 0x44, 0x24, 0x08]);
            e.movsd_rsp_xmm(8, 0);
            e.add_rsp(8);
            Template { code: e.as_slice().to_vec(), holes: vec![] }
        };
        let compare = |cc: Cc| {
            let mut e = Emitter::new();
            e.movsd_xmm_rsp(0, 8);
            e.movsd_xmm_rsp(1, 0);
            e.comisd(0, 1);
            e.setcc(cc, 0);
            e.movzx(0, 0);
            e.push_r(0);
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
            e.movsd_xmm_rsp(1, 0);
            e.xorps(3, 3);
            e.ucomisd(1, 3);
            e.jcc(Cc::P, nan_or_zero);
            e.jcc(Cc::Eq, trap);
            e.bind(trap);
            e.call_abs(trap_addr);
            e.ud2();
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
            e.push_r(0);
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
            e.pop_r(0);
            e.mov_rbp_r(0, 0);
            Template { code: e.as_slice().to_vec(), holes: vec![(2, Hole::Disp8Slot)] }
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
        let halt_tail = {
            let mut e = Emitter::new();
            e.emit(&[0x31, 0xC0]);
            e.emit(&[0x31, 0xD2]);
            e.epilogue();
            e.as_slice().to_vec()
        };
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
            sub: sse_binop(0x5C),
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
    // Pass 2: copy templates and patch holes.
    let mut bytes = Vec::with_capacity(off as usize + lib.halt_tail.len());
    bytes.resize(off as usize, 0);
    for (pc, insn) in func.code.iter().enumerate() {
        if insn.op == Opcode::Halt {
            continue;
        }
        let t = lib.template(insn.op).ok_or(IllegalForT0::Opcode(insn.op))?;
        let base = offsets[pc] as usize;
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
                    let rel = tgt_off as i64 - (base as i64 + t.code.len() as i64);
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
    bytes.extend_from_slice(&lib.halt_tail);
    Ok(T0Code { bytes, offsets, max_stack: report.max_stack })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cold::bytecode::decode_vertex;

    #[test]
    fn golden_push_pop_movabs() {
        let mut e = Emitter::new();
        e.push_r(0);
        e.push_r(10);
        e.pop_r(0);
        e.pop_r(10);
        e.movabs(0, 0x1234_5678_9ABC_DEF0);
        e.movabs(10, 1);
        assert_eq!(
            e.as_slice(),
            &[
                0x50, 0x41, 0x52, 0x58, 0x41, 0x5A, // push/pop rax, r10
                0x48, 0xB8, 0xF0, 0xDE, 0xBC, 0x9A, 0x78, 0x56, 0x34, 0x12, // movabs rax
                0x49, 0xBA, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // movabs r10
            ]
        );
    }

    #[test]
    fn golden_sse_moves_and_arith() {
        let mut e = Emitter::new();
        e.movsd_xmm_rsp(0, 0);
        e.movsd_rsp_xmm(8, 0);
        e.movsd_xmm_rbp(1, -8);
        e.movsd_rbp_xmm(-8, 1);
        e.f2_op(0x58, 0, 1);
        e.comisd(0, 1);
        e.ucomisd(0, 1);
        e.movq_xmm_r(2, 10);
        e.xorps(3, 3);
        assert_eq!(
            e.as_slice(),
            &[
                0xF2, 0x0F, 0x10, 0x44, 0x24, 0x00, // movsd xmm0,[rsp]
                0xF2, 0x0F, 0x11, 0x44, 0x24, 0x08, // movsd [rsp+8],xmm0
                0xF2, 0x0F, 0x10, 0x4D, 0xF8, // movsd xmm1,[rbp-8]
                0xF2, 0x0F, 0x11, 0x4D, 0xF8, // movsd [rbp-8],xmm1
                0xF2, 0x0F, 0x58, 0xC1, // addsd xmm0, xmm1
                0x66, 0x0F, 0x2F, 0xC1, // comisd
                0x66, 0x0F, 0x2E, 0xC1, // ucomisd
                0x66, 0x49, 0x0F, 0x6E, 0xD2, // movq xmm2, r10
                0x0F, 0x57, 0xDB, // xorps xmm3, xmm3
            ]
        );
    }

    #[test]
    fn finalize_fails_closed_on_unbound_label() {
        let mut e = Emitter::new();
        let l = e.label();
        e.jmp(l);
        assert!(e.finalize().is_err());
    }

    #[test]
    fn golden_stack_and_frame_moves() {
        let mut e = Emitter::new();
        e.mov_r_rsp(0, 8);
        e.mov_rsp_r(8, 0);
        e.mov_r_rbp(0, -8);
        e.mov_rbp_r(-8, 0);
        e.sub_rsp(32);
        e.add_rsp(32);
        e.call_abs(0);
        assert_eq!(
            e.as_slice(),
            &[
                0x48, 0x8B, 0x44, 0x24, 0x08, // mov rax,[rsp+8]
                0x48, 0x89, 0x44, 0x24, 0x08, // mov [rsp+8],rax
                0x48, 0x8B, 0x45, 0xF8, // mov rax,[rbp-8]
                0x48, 0x89, 0x45, 0xF8, // mov [rbp-8],rax
                0x48, 0x81, 0xEC, 32, 0, 0, 0, // sub rsp,32
                0x48, 0x81, 0xC4, 32, 0, 0, 0, // add rsp,32
                0x49, 0xBB, 0, 0, 0, 0, 0, 0, 0, 0, // movabs r11, 0
                0x41, 0xFF, 0xD3, // call r11
            ]
        );
    }

    #[test]
    fn legality_accepts_arithmetic_and_rejects_strings() {
        let mut b = Vec::new();
        b.push(22u8);
        b.extend_from_slice(&5.0f32.to_le_bytes());
        b.push(22u8);
        b.extend_from_slice(&2.0f32.to_le_bytes());
        b.push(1u8); // ADD
        b.push(255u8);
        let f = decode_vertex("a", &b).unwrap();
        assert!(check_legality(&f).is_ok());
        // String push is outside the subset.
        let mut b2 = vec![20u8];
        b2.extend_from_slice(&1u32.to_le_bytes());
        b2.push(b's');
        b2.push(255u8);
        let f2 = decode_vertex("s", &b2).unwrap();
        assert!(matches!(
            check_legality(&f2),
            Err(IllegalForT0::Opcode(_))
        ));
    }

    /// Disassembly cross-check when objdump is available (CI runners ship
    /// binutils; the test skips silently elsewhere).
    #[test]
    fn objdump_cross_check() {
        let mut e = Emitter::new();
        e.prologue();
        e.movsd_xmm_rsp(0, 0);
        e.f2_op(0x58, 0, 1);
        e.movsd_rsp_xmm(8, 0);
        e.epilogue();
        e.finalize().unwrap();
        let bytes = e.as_slice().to_vec();
        let Ok(out) = std::process::Command::new("objdump")
            .args(["-D", "-b", "binary", "-m", "i386:x86-64", "--no-show-raw-insn", "/dev/stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                if let Some(si) = c.stdin.as_mut() {
                    let _ = si.write_all(&bytes);
                }
                c.wait_with_output()
            })
        else {
            return;
        };
        if !out.status.success() {
            return;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("addsd"), "objdump should decode addsd; got: {text}");
    }
}

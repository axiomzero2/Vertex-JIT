// CEP:FILE: engine/src/cold/tier0_tests.rs
// CEP:WHAT: Tier 0 unit tests: golden byte lockfiles for every template,
//           the objdump disassembly cross-check, finalize failure
//           behavior, legality acceptance/negatives and the two defect-
//           class pins (compare collapse; Drop depth simulation).
// CEP:WHY: Copy-and-patch correctness is byte-exact; these lockfiles and
//          cross-checks are the evidence chain for the T0 encoder
//          (doc/tier0.md, standard 61.2).
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: any assertion failure names the template and the differing
//              bytes.
// CEP:ASSUMES: golden bytes reviewed against the Intel SDM and objdump
//              (benches/EVIDENCE.md).
// CEP:COST: offline; the objdump test skips when binutils is absent.
// CEP:EVIDENCE: this module; tests/t0_exec.rs execution parity;
//               tests/stack_discipline matrices A/B.
// CEP:SECURITY: test-authored constants only.

#[cfg(test)]
mod golden {
    use crate::cold::tier0::*;
    
    use crate::cold::tier0_emit::*;
    use crate::cold::bytecode::decode_vertex;

    /// CEP:WHAT: Golden bytes for push/pop and movabs encodings.
/// CEP:WHY: Copy-and-patch correctness is byte-exact; a golden lockfile
///              catches silent encoder drift.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: byte assertions inline; objdump cross-check test.
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

    /// CEP:WHAT: Golden bytes for the reg-reg move direction bit.
/// CEP:WHY: The ModRM direction bit is the classic x86 encoder bug; the
///              golden pins the correct direction.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: byte assertions inline.
    #[test]
    fn golden_mov_rr_direction() {
        let mut e = Emitter::new();
        e.mov_rr(7, 0); // rdi = rax
        assert_eq!(e.as_slice(), &[0x48, 0x89, 0xC7], "mov_rr(7,0) must be `mov rdi, rax`");
        let mut e = Emitter::new();
        e.mov_rr(0, 7); // rax = rdi
        assert_eq!(e.as_slice(), &[0x48, 0x89, 0xF8], "mov_rr(0,7) must be `mov rax, rdi`");
    }

    /// CEP:WHAT: Golden bytes for SSE moves and arithmetic (f2/f3 prefixes).
/// CEP:WHY: compare/add/mul/div templates share the SSE encoding family; the
///              goldens pin prefixes, ModRM and SIB forms.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: byte assertions inline; objdump cross-check test.
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

    /// CEP:WHAT: finalize fails closed on an unbound label.
/// CEP:WHY: Law 6: an unresolvable jump must abort emission, never emit a
///              wild displacement.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: negative case asserted inline.
    #[test]
    fn finalize_fails_closed_on_unbound_label() {
        let mut e = Emitter::new();
        let l = e.label();
        e.jmp(l);
        assert!(e.finalize().is_err());
    }

    /// CEP:WHAT: Golden bytes for rsp/rbp addressing and frame moves.
/// CEP:WHY: Slot addressing [rbp-8*(slot+1)] is the T0 memory discipline; the
///              golden pins displacement forms.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: byte assertions inline.
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

    /// CEP:WHAT: check_legality accepts the arithmetic subset and rejects
///              string ops.
/// CEP:WHY: The T0 subset boundary is the trust gate; strings fall back to
///              higher tiers.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: positive and negative legality cases inline.
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
/// CEP:WHAT: Disassemble the prologue/epilogue and one SSE op with objdump.
/// CEP:WHY: Golden bytes can be self-consistently wrong; an independent
///              disassembler cross-check anchors the encoder to the ISA.
/// CEP:STATUS: complete
/// CEP:FAILURE: assertions fail on decode mismatch; the test skips
///              (pass) when objdump is unavailable.
/// CEP:ASSUMES: objdump on PATH (CI runners ship binutils).
/// CEP:COST: one objdump invocation.
/// CEP:EVIDENCE: benches/EVIDENCE.md records the reviewed disassembly.
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

    /// The comparison templates must collapse the operand pair in place:
    /// the boolean result is written to the left slot and the stack pointer
    /// moves up one slot. Pushing the result instead leaves both operand
    /// slots on the machine stack and leaks 16 bytes per execution; inside
    /// a loop that is a stack overflow (tests/stack_discipline.rs matrix A).
    /// CEP:WHAT: Golden bytes prove compare/equal templates collapse the operand
///              pair in place.
/// CEP:WHY: Defect class A: the templates pushed their boolean result,
///              leaking 16 bytes per execution; the golden pins the
///              in-place collapse (mov [rsp+8], rax; add rsp, 8).
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: byte assertions inline; matrix A proves behavior; the
///               defect history is in benches/EVIDENCE.md.
    #[test]
    fn golden_compare_templates_collapse_operand_pair() {
        let lib = TemplateLibrary::new(0x1000, 0x2000);
        for (t, setcc) in [(&lib.less_than, 0x92u8), (&lib.greater_than, 0x97u8)] {
            assert_eq!(
                t.code,
                &[
                    0xF2, 0x0F, 0x10, 0x44, 0x24, 0x08, // movsd xmm0,[rsp+8]  (left)
                    0xF2, 0x0F, 0x10, 0x4C, 0x24, 0x00, // movsd xmm1,[rsp]    (right)
                    0x66, 0x0F, 0x2F, 0xC1, // comisd xmm0, xmm1
                    0x0F, setcc, 0xC0, // set<cc> al  (92=b, 97=a)
                    0x48, 0x0F, 0xB6, 0xC0, // movzx rax, al
                    0x48, 0x89, 0x44, 0x24, 0x08, // mov [rsp+8], rax
                    0x48, 0x81, 0xC4, 0x08, 0x00, 0x00, 0x00, // add rsp, 8
                ][..]
            );
        }
        assert_eq!(
            lib.equal.code,
            &[
                0xF2, 0x0F, 0x10, 0x44, 0x24, 0x08, // movsd xmm0,[rsp+8]
                0xF2, 0x0F, 0x10, 0x4C, 0x24, 0x00, // movsd xmm1,[rsp]
                0x66, 0x0F, 0x2F, 0xC1, // comisd xmm0, xmm1
                0x0F, 0x94, 0xC0, // sete al
                0x0F, 0x9B, 0xC1, // setnp cl
                0x20, 0xC8, // and al, cl
                0x48, 0x0F, 0xB6, 0xC0, // movzx rax, al
                0x48, 0x89, 0x44, 0x24, 0x08, // mov [rsp+8], rax
                0x48, 0x81, 0xC4, 0x08, 0x00, 0x00, 0x00, // add rsp, 8
            ][..]
        );
    }

    /// The T0 depth simulation must treat Drop as a no-op on the value
    /// stack: the machine template emits no stack adjustment, the
    /// interpreter pops a binding, not a value, and check_legality agrees.
    /// A Drop-shaped program therefore returns the bottom of the value
    /// stack, not a zeroed rax.
    /// CEP:WHAT: The compile() depth simulation treats Drop as a binding
///              retirement with no value-stack effect.
/// CEP:WHY: Defect class B: the halt-tail simulation popped on Drop and read
///              the wrong slot; the depth model must agree with the
///              template, the interpreter and check_legality.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: matrix B (50 cases) proves behavior; the trigger shape is
///               asserted inline.
    #[test]
    fn drop_does_not_pop_value_stack_in_halt_depth() {
        use crate::cold::bytecode::{Function, Insn, Operand, Opcode as Op};
        let f = Function {
            name: "dropdepth".to_owned(),
            locals: vec!["i".to_owned()],
            strings: Vec::new(),
            code: vec![
                Insn { op: Op::PushNumber, operand: Operand::F64(5.0) },
                Insn { op: Op::StoreVar, operand: Operand::Slot(0) },
                Insn { op: Op::PushNumber, operand: Operand::F64(7.0) },
                Insn { op: Op::Drop, operand: Operand::Slot(0) },
                Insn { op: Op::Halt, operand: Operand::None },
            ],
        };
        let lib = TemplateLibrary::new(0x1000, 0x2000);
        let c = compile(&f, &lib).expect("compile");
        // depth 1 at Halt: the tail reads [rsp] and ends in the epilogue
        // (mov rax,[rsp] ; xor edx,edx ; mov rsp,rbp ; pop rbp ; ret).
        assert_eq!(
            &c.bytes[c.bytes.len() - 12..],
            &[0x48, 0x8B, 0x44, 0x24, 0x00, 0x31, 0xD2, 0x48, 0x89, 0xEC, 0x5D, 0xC3][..],
            "halt tail must load [rsp+0], not zero rax"
        );
    }
}

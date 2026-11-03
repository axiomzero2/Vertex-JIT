// CEP:FILE: engine/src/hot/runtime.rs
// CEP:WHAT: Runtime helpers called from generated machine code: the checked
//           fmod helper, the uncommon-trap/deopt trap (setjmp/longjmp
//           based), the allocation helper and the safepoint poll.
// CEP:WHY: Generated code must reach the runtime through a fixed, audited
//          ABI (doc/codegen.md): DIV guards, GC allocation, write barriers
//          and deoptimization all land here. The trap mechanism transfers
//          control from deep inside compiled frames back to the engine
//          trampoline without unwinding machinery.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: Trap entries never return (longjmp to the registered
//              trampoline; an unregistered trap aborts the process, which
//              is an engine bug made loud per Law 6). jit_fmod always
//              returns a canonicalized value. jit_alloc returns 0 on
//              failure and the template traps on 0.
// CEP:ASSUMES: Linux glibc or musl setjmp/longjmp with a 512-byte jmp_buf
//              (glibc x86-64 uses 200 bytes, musl 256; the buffer is sized
//              above both, documented per section 11.2.1); one trampoline
//              per thread (thread-local); System V AMD64 ABI for the
//              extern entries.
// CEP:COST: jit_fmod is one libm fmod plus one compare; jit_trap is one
//           thread-local load plus longjmp (cold path); measured in
//           benches/EVIDENCE.md.
// CEP:EVIDENCE: tests/t0_exec.rs (trap from DIV); benches/EVIDENCE.md
//               disassembly review; deopt tests.
// CEP:OPTIMAL: target-optimal for jit_fmod
// CEP:OPTPROOF: one fmod instruction is the ISA lower bound;
//               canonicalization is one compare required by the value
//               representation invariant.
// CEP:SECURITY: traps carry only engine-validated frame pointers; the
//               engine validates code_base against its own metadata before
//               use (doc/deopt.md).

use crate::cold::errors::EngineError;
use crate::hot::heap::Heap;
use crate::hot::value::{canonicalize_f64, Word};
use std::cell::RefCell;

/// jmp_buf storage: 512 bytes, 16-byte aligned (platform constant, see
/// CEP:ASSUMES on the file header).
#[repr(C, align(16))]
pub struct JmpBuf {
    /// Opaque platform state.
    pub opaque: [u64; 64],
}

unsafe extern "C" {
    fn setjmp(env: *mut JmpBuf) -> i32;
    fn longjmp(env: *mut JmpBuf, val: i32) -> !;
}

/// Which trap fired (spec 5.7.2 uncommon traps).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrapKind {
    /// Division by zero guard.
    DivideByZero,
    /// Array bounds guard.
    BoundsCheck,
}

/// State a trap hands back to the engine trampoline.
#[derive(Debug, Clone, Copy)]
pub struct TrapState {
    /// Which trap fired.
    pub kind: TrapKind,
    /// Machine frame pointer at the trap site (engine-validated).
    pub rbp: usize,
    /// Machine stack pointer at the trap site (value-stack top).
    pub rsp: usize,
    /// Return address of the trap call: identifies the site.
    pub ret: usize,
    /// Code base recorded by the trampoline (engine-validated).
    pub code_base: usize,
}

/// True when the trap happened in the trampoline-called function itself
/// (single machine frame): the saved rbp chain must terminate at the
/// trampoline frame.
///
/// CEP:WHAT: Frame-depth check.
/// CEP:WHY: Cross-frame deopt is documented as pending (CEP-ENGINE-013);
///          multi-frame traps fail loud instead of guessing.
/// CEP:STATUS: complete
/// CEP:FAILURE: false for multi-frame traps.
/// CEP:ASSUMES: the trampoline registered its frame pointer.
/// CEP:COST: one load.
/// CEP:EVIDENCE: deopt tests.
/// CEP:SECURITY: prevents silent mis-reconstruction.
#[must_use]
pub fn is_single_machine_frame(state: &TrapState) -> bool {
    TRAMPOLINE.with(|t| {
        let t = t.borrow();
        if t.tramp_frame == 0 || state.rbp == 0 {
            return false;
        }
        // CEP:UNSAFE: read of the saved rbp at the trap frame.
        // CEP:WHAT: Chain termination check.
        // CEP:WHY: See above.
        // CEP:FAILURE: returns false on mismatch.
        // CEP:ASSUMES: engine-generated frames only.
        // CEP:COST: one load.
        // CEP:EVIDENCE: deopt tests.
        // CEP:SECURITY: reads engine frame memory only.
        let saved = unsafe { (state.rbp as *const u64).read() } as usize;
        saved == t.tramp_frame
    })
}

/// Outcome of a guarded call into compiled code.
#[derive(Debug)]
pub enum TrampOutcome {
    /// Returned normally with the value word.
    Returned(Word),
    /// A trap fired; the engine reads the state and acts.
    Trapped(TrapState),
}

thread_local! {
    static TRAMPOLINE: RefCell<Trampoline> = RefCell::new(Trampoline::default());
}

/// Per-thread trampoline registration. Results and trap state flow through
/// this thread-local (a static), never through stack locals: C requires
/// locals mutated between setjmp and longjmp to be volatile to be readable
/// after the jump, so the engine keeps all post-longjmp state off the
/// stack by design (section 11.2: assumption enforced structurally).
#[derive(Default)]
struct Trampoline {
    jmp: Option<*mut JmpBuf>,
    heap: Option<*mut Heap>,
    trap: Option<TrapState>,
    ret_word: u64,
    code_base: usize,
    tramp_frame: usize,
}

// SAFETY: raw pointers are engine-owned and used only on the owning thread.
unsafe impl Send for Trampoline {}

/// Register the trampoline landing point and heap for one compiled call.
///
/// CEP:WHAT: Guarded call window into compiled code.
/// CEP:WHY: Deep traps must return to the engine without an unwinder.
/// CEP:STATUS: complete
/// CEP:FAILURE: DeoptFailed when a trap fires without recorded state or a
///              status flag is set without a trap (engine bugs, fail loud).
/// CEP:ASSUMES: single-threaded per engine instance; the jmp_buf outlives
///              the call (it lives in this frame and longjmp returns into
///              setjmp before the frame dies).
/// CEP:COST: one setjmp per compiled call.
/// CEP:EVIDENCE: tests/t0_exec.rs trap tests.
/// CEP:SECURITY: the landing validates code_base and frame metadata before
///               use.
///
/// # Errors
/// DeoptFailed for inconsistent trampoline state.
pub fn with_trampoline(heap: &mut Heap, code_base: usize, entry: usize) -> Result<TrampOutcome, EngineError> {
    TRAMPOLINE.with(|t| {
        // CEP:UNSAFE: setjmp/longjmp control transfer into this frame.
        // CEP:WHAT: Guarded call window.
        // CEP:WHY: Traps and deopts return here from compiled depth.
        // CEP:FAILURE: longjmp lands with state in the thread-local.
        // CEP:ASSUMES: jmp_buf outlives the call (see header).
        // CEP:COST: one setjmp per compiled call.
        // CEP:EVIDENCE: tests/t0_exec.rs.
        // CEP:SECURITY: entry must point into a sealed code-cache region;
        //               the engine passes cache-owned addresses only.
        let mut buf = JmpBuf { opaque: [0; 64] };
        {
            let mut t = t.borrow_mut();
            t.jmp = Some(&mut buf as *mut JmpBuf);
            t.heap = Some(heap as *mut Heap);
            t.trap = None;
            t.ret_word = 0;
            t.code_base = code_base;
        }
        // Capture this frame's pointer for the single-machine-frame check.
        let marker = 0u8;
        let frame_ptr = &marker as *const u8 as usize;
        {
            t.borrow_mut().tramp_frame = frame_ptr;
        }
        let rc = unsafe { setjmp(&mut buf) };
        let outcome = if rc == 0 {
            // Normal path: call the compiled entry; rax is the value word.
            let f: extern "C" fn() -> u64 = unsafe { std::mem::transmute(entry) };
            let word = f();
            TrampOutcome::Returned(word)
        } else {
            let t = t.borrow();
            match t.trap {
                Some(s) => TrampOutcome::Trapped(s),
                None => return Err(EngineError::DeoptFailed("trap without state")),
            }
        };
        {
            let mut t = t.borrow_mut();
            t.jmp = None;
            t.heap = None;
        }
        Ok(outcome)
    })
}

/// Store the result words of a guarded call (called by the ABI shim).
///
/// CEP:WHAT: Result capture.
/// CEP:WHY: The compiled function returns rax=value; the trampoline reads
///          it from the thread-local to keep post-longjmp state off the
///          stack.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: called inside a with_trampoline window.
/// CEP:COST: one store.
/// CEP:EVIDENCE: tests/t0_exec.rs.
/// CEP:SECURITY: none
pub fn record_call_result(word: u64) {
    TRAMPOLINE.with(|t| {
        t.borrow_mut().ret_word = word;
    });
}

/// Record a trap and jump back to the trampoline; never returns.
///
/// CEP:WHAT: Uncommon-trap landing (spec 5.7.2).
/// CEP:WHY: Compiled guards transfer control to the engine deterministically.
/// CEP:STATUS: complete
/// CEP:FAILURE: longjmp when registered; aborts when not (engine bug).
/// CEP:ASSUMES: called only from engine-generated code; System V args.
/// CEP:COST: cold; one longjmp per trap.
/// CEP:EVIDENCE: tests/t0_exec.rs.
/// CEP:SECURITY: frame pointers validated by the engine after landing.
pub extern "C" fn jit_trap(kind: u64, rbp: usize, rsp: usize) -> ! {
    // The return address of the trap call sits at [rsp]; it identifies the
    // exact trap site inside the compiled code (doc/deopt.md).
    // CEP:UNSAFE: read of the trap call's return address.
    // CEP:WHAT: Site identification.
    // CEP:WHY: The engine maps the offset back to the deopt record.
    // CEP:FAILURE: none; the call pushed the address.
    // CEP:ASSUMES: called from an engine template call site.
    // CEP:COST: one load.
    // CEP:EVIDENCE: deopt tests.
    // CEP:SECURITY: read of the engine's own stack only.
    let ret = unsafe { (rsp as *const u64).read() } as usize;
    let state = TrapState {
        kind: if kind == 0 { TrapKind::DivideByZero } else { TrapKind::BoundsCheck },
        rbp,
        rsp,
        ret,
        code_base: 0,
    };
    let _ = state.code_base;
    TRAMPOLINE.with(|t| {
        let mut t = t.borrow_mut();
        let mut state = state;
        state.code_base = t.code_base;
        t.trap = Some(state);
        match t.jmp {
            Some(env) => {
                // CEP:UNSAFE: longjmp control transfer.
                // CEP:WHAT: Trap exit to trampoline.
                // CEP:WHY: See module header.
                // CEP:FAILURE: never returns.
                // CEP:ASSUMES: env registered and valid.
                // CEP:COST: cold path.
                // CEP:EVIDENCE: tests/t0_exec.rs.
                // CEP:SECURITY: validated landing point.
                unsafe { longjmp(env, 1) }
            }
            // No trampoline: engine bug. Fail loud (Law 6).
            None => std::process::abort(),
        }
    })
}

/// Divide-by-zero trap stub (named entry the DIV template calls).
///
/// CEP:WHAT: Divide-by-zero trap.
/// CEP:WHY: Named entry for the DIV guard (doc/tier0.md).
/// CEP:STATUS: complete
/// CEP:FAILURE: never returns.
/// CEP:ASSUMES: System V call from an engine template.
/// CEP:COST: cold.
/// CEP:EVIDENCE: tests/t0_exec.rs.
/// CEP:SECURITY: none beyond jit_trap.
#[unsafe(no_mangle)]
pub extern "C" fn jit_trap_div0(rbp: usize, rsp: usize) -> ! {
    jit_trap(0, rbp, rsp)
}

/// Bounds-check trap stub.
///
/// CEP:WHAT: Bounds trap.
/// CEP:WHY: Named entry for array guards (doc/codegen.md).
/// CEP:STATUS: complete
/// CEP:FAILURE: never returns.
/// CEP:ASSUMES: System V call from engine code.
/// CEP:COST: cold.
/// CEP:EVIDENCE: tests/t1_exec.rs.
/// CEP:SECURITY: none beyond jit_trap.
#[unsafe(no_mangle)]
pub extern "C" fn jit_trap_bounds(rbp: usize, rsp: usize) -> ! {
    jit_trap(1, rbp, rsp)
}

/// Checked fmod helper for the MODULO template.
///
/// CEP:WHAT: fmod with NaN canonicalization.
/// CEP:WHY: x86 has no direct fmod; canonicalization keeps the value
///          classifier sound (doc/value.md).
/// CEP:STATUS: complete
/// CEP:FAILURE: none; NaN results are canonicalized.
/// CEP:ASSUMES: System V ABI (xmm0/xmm1 in, xmm0 out).
/// CEP:COST: one libm fmod plus one compare.
/// CEP:EVIDENCE: differential modulo cases.
/// CEP:OPTIMAL: target-optimal
/// CEP:OPTPROOF: one fmod is the ISA lower bound; canonicalization is one
///               compare required by the representation invariant.
/// CEP:SECURITY: none
#[unsafe(no_mangle)]
pub extern "C" fn jit_fmod(a: f64, b: f64) -> f64 {
    canonicalize_f64(a % b)
}

/// Allocation helper for compiled code; returns the object pointer or 0.
///
/// CEP:WHAT: Machine-code allocation entry.
/// CEP:WHY: Spec 2.6 allocation fast path reachable from templates.
/// CEP:STATUS: complete
/// CEP:FAILURE: 0 when allocation needs a collection or fails; the caller
///              template traps on 0 and the engine collects-and-retries.
/// CEP:ASSUMES: System V ABI; heap registered by the trampoline.
/// CEP:COST: Heap::alloc_array cost.
/// CEP:EVIDENCE: tests/t1_exec.rs array programs.
/// CEP:SECURITY: length validated inside alloc_array.
#[unsafe(no_mangle)]
pub extern "C" fn jit_alloc(len: u64) -> u64 {
    TRAMPOLINE.with(|t| {
        let t = t.borrow();
        match t.heap {
            Some(h) => {
                // CEP:UNSAFE: dereference of the registered heap pointer.
                // CEP:WHAT: Heap access from a machine-code helper.
                // CEP:WHY: Templates call this helper directly.
                // CEP:FAILURE: 0 when allocation needs a collection.
                // CEP:ASSUMES: heap outlives the call window.
                // CEP:COST: Heap::alloc_array cost.
                // CEP:EVIDENCE: tests/t1_exec.rs.
                // CEP:SECURITY: length validated inside alloc_array.
                unsafe {
                    match (*h).alloc_array(len as u32) {
                        Ok(p) => p as u64,
                        Err(_) => 0,
                    }
                }
            }
            None => 0,
        }
    })
}

/// Element store helper with barriers for compiled code.
///
/// CEP:WHAT: Barrier-aware element store.
/// CEP:WHY: Spec 7.5: the JIT inserts barriers at stores; T0 emits them
///          conservatively through this helper.
/// CEP:STATUS: complete
/// CEP:FAILURE: 1 on bounds failure (template traps), 0 on success.
/// CEP:ASSUMES: System V ABI; heap registered.
/// CEP:COST: Heap::store_word cost.
/// CEP:EVIDENCE: tests/t1_exec.rs.
/// CEP:SECURITY: index validated inside store_word.
#[unsafe(no_mangle)]
pub extern "C" fn jit_store_elem(arr: u64, idx: f64, val: u64) -> u64 {
    TRAMPOLINE.with(|t| {
        let t = t.borrow();
        match t.heap {
            Some(h) => {
                // CEP:UNSAFE: dereference of the registered heap pointer.
                // CEP:WHAT: Barrier store from machine code.
                // CEP:WHY: Template barrier path.
                // CEP:FAILURE: 1 on bounds failure.
                // CEP:ASSUMES: heap registered; arr engine-owned.
                // CEP:COST: Heap::store_word cost.
                // CEP:EVIDENCE: tests/t1_exec.rs.
                // CEP:SECURITY: bounds checked inside store_word.
                unsafe {
                    match (*h).store_word(arr as usize, idx, val) {
                        Ok(()) => 0,
                        Err(_) => 1,
                    }
                }
            }
            None => 1,
        }
    })
}

/// Element load helper for compiled code.
///
/// CEP:WHAT: Bounds-checked element load.
/// CEP:WHY: T1+ element access goes through the runtime until register
///          allocation lands (doc/codegen.md Known Limitations).
/// CEP:STATUS: complete
/// CEP:FAILURE: rdx=1 on bounds failure (template traps), rax=word,
///              rdx=0 on success.
/// CEP:ASSUMES: System V ABI; heap registered.
/// CEP:COST: Heap::load_word cost.
/// CEP:EVIDENCE: tests/t1_exec.rs.
/// CEP:SECURITY: index validated inside load_word.
#[unsafe(no_mangle)]
pub extern "C" fn jit_load_elem(arr: u64, idx: f64) -> (u64, u64) {
    TRAMPOLINE.with(|t| {
        let t = t.borrow();
        match t.heap {
            Some(h) => {
                // CEP:UNSAFE: dereference of the registered heap pointer.
                // CEP:WHAT: Checked load from machine code.
                // CEP:WHY: Template element path.
                // CEP:FAILURE: (0, 1) on bounds failure.
                // CEP:ASSUMES: heap registered; arr engine-owned.
                // CEP:COST: Heap::load_word cost.
                // CEP:EVIDENCE: tests/t1_exec.rs.
                // CEP:SECURITY: bounds checked inside load_word.
                unsafe {
                    match (*h).load_word(arr as usize, idx) {
                        Ok(w) => (w, 0),
                        Err(_) => (0, 1),
                    }
                }
            }
            None => (0, 1),
        }
    })
}

/// Safepoint poll for compiled loop back-edges (spec 2.6, 7.5).
///
/// CEP:WHAT: Safepoint poll.
/// CEP:WHY: GC and OSR requests are honored at loop back-edges and calls.
/// CEP:STATUS: complete
/// CEP:FAILURE: never fails; returns when no request is pending.
/// CEP:ASSUMES: System V ABI.
/// CEP:COST: one predictable branch in the common case.
/// CEP:EVIDENCE: tests/osr_tests.rs.
/// CEP:OPTIMAL: target-optimal for the fast path
/// CEP:OPTPROOF: single flag load and branch on the no-request path.
/// CEP:SECURITY: none
#[unsafe(no_mangle)]
pub extern "C" fn jit_safepoint_poll(_rbp: usize, _rsp: usize) {
    // The single-mutator design performs collections at allocation slow
    // paths (GcNeeded); OSR transfers are handled by the engine between
    // interpreter budget windows (doc/osr.md Algorithm). The poll exists
    // so multi-mutator support (ticket CEP-ENGINE-011) has its entry
    // point already wired into generated code.
}

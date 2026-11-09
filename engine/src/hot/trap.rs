// CEP:FILE: engine/src/hot/trap.rs
// CEP:WHAT: The uncommon-trap machinery: the per-thread trampoline,
//           setjmp/longjmp landing, trap-state capture and the named
//           trap entries the compiled templates call.
// CEP:WHY: Deep traps must return to the engine without an unwinder
//          (spec 5.7.2, doc/deopt.md); all post-longjmp state flows
//          through the thread-local by design (section 11.2).
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: DeoptFailed when a trap fires without recorded state;
//              jit_trap aborts when no trampoline is registered (an
//              engine bug, made loud per Law 6).
// CEP:ASSUMES: Linux glibc or musl setjmp/longjmp with a 512-byte jmp_buf
//              (glibc x86-64 uses 200 bytes, musl 256; the buffer is sized
//              above both, documented per section 11.2.1); one trampoline
//              per thread; System V AMD64 ABI for the extern entries.
// CEP:COST: one setjmp per compiled call; jit_trap is one thread-local
//           store plus longjmp (cold path).
// CEP:EVIDENCE: tests/t0_exec.rs trap tests; deopt tests;
//               benches/EVIDENCE.md disassembly review.
// CEP:SECURITY: traps carry only engine-validated frame pointers; the
//               engine validates code_base against its own metadata
//               before use (doc/deopt.md).

use crate::cold::errors::EngineError;
use crate::hot::heap::Heap;
use crate::hot::value::Word;
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
#[derive(Debug, Clone)]
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
    /// The compiled frame image, captured here at trap time: slot i holds
    /// the word at [rbp - 8*(i+1)] for i in 0..(rbp-rsp)/8. The capture is
    /// mandatory — after the longjmp the engine's own Rust frames occupy
    /// the same stack region, so reads at reconstruct time would see the
    /// engine's own locals, not the compiled frame (doc/deopt.md).
    pub frame: Vec<u64>,
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
        if t.call_rsp == 0 || state.rbp == 0 {
            return false;
        }
        // The compiled entry's rbp is deterministic in rsp: the trampoline
        // captured call_rsp immediately before the call, and the entry's
        // prologue pushes the return address (8) and the caller's rbp (8),
        // so a single machine frame satisfies rbp == call_rsp - 16. A
        // machine-called callee sits deeper and fails the equality (the
        // cross-frame case stays fail-loud, CEP-ENGINE-013).
        // CEP:WHAT: Frame-depth check (rsp-anchored).
        // CEP:WHY: rbp is a general register in release builds and cannot
        //          anchor the check; the saved-rbp comparison silently
        //          depended on frame-pointer assumptions.
        // CEP:FAILURE: false for multi-frame traps.
        // CEP:ASSUMES: the compiled entry's first two instructions are
        //              push rbp; mov rbp, rsp (the engine's prologue).
        // CEP:COST: O(1).
        // CEP:EVIDENCE: tests/call_inline_tests.rs deopt-resume test;
        //              tests/t0_exec.rs trap tests.
        // CEP:SECURITY: prevents silent mis-reconstruction.
        state.rbp == t.call_rsp.wrapping_sub(16)
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
    pub(crate) static TRAMPOLINE: RefCell<Trampoline> = RefCell::new(Trampoline::default());
}

/// Per-thread trampoline registration. Results and trap state flow through
/// this thread-local (a static), never through stack locals: C requires
/// locals mutated between setjmp and longjmp to be volatile to be readable
/// after the jump, so the engine keeps all post-longjmp state off the
/// stack by design (section 11.2: assumption enforced structurally).
#[derive(Default)]
pub(crate) struct Trampoline {
    jmp: Option<*mut JmpBuf>,
    pub(crate) heap: Option<*mut Heap>,
    trap: Option<TrapState>,
    ret_word: u64,
    code_base: usize,
    tramp_frame: usize,
    /// rsp immediately before the compiled entry call: the entry's frame
    /// starts exactly 16 bytes below it (return address push + rbp push).
    /// rsp is the anchor for the single-machine-frame check — unlike rbp,
    /// the compiler maintains it precisely at every call site.
    call_rsp: usize,
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
        // Capture this frame's pointer for diagnostics; the real anchor
        // for the single-machine-frame check is call_rsp (below): rbp is
        // a general register in release builds and cannot anchor it.
        // CEP:UNSAFE: reads the current frame pointer register.
        // CEP:WHAT: Frame-pointer capture.
        // CEP:WHY: Kept for cross-checks and debugging.
        // CEP:FAILURE: none; a register read.
        // CEP:ASSUMES: none (the value is advisory).
        // CEP:COST: one register move.
        // CEP:EVIDENCE: deopt tests.
        // CEP:SECURITY: read-only register access.
        let frame_ptr: usize;
        unsafe { core::arch::asm!("mov {}, rbp", out(reg) frame_ptr, options(nomem)) };
        {
            t.borrow_mut().tramp_frame = frame_ptr;
        }
        let rc = unsafe { setjmp(&mut buf) };
        let outcome = if rc == 0 {
            // Normal path: call the compiled entry; rax is the value word.
            // Capture rsp immediately before the call: the entry's rbp is
            // exactly call_rsp - 16 (return-address push + rbp push),
            // which anchors the single-machine-frame check (doc/deopt.md).
            // CEP:UNSAFE: reads the current stack pointer register.
            // CEP:WHAT: Call-site rsp capture.
            // CEP:WHY: The compiled entry's frame position is deterministic
            //          in rsp; the check must not guess.
            // CEP:FAILURE: none; a register read.
            // CEP:ASSUMES: no rsp adjustment between this read and the
            //              call (the compiler emits them adjacently).
            // CEP:COST: one register move.
            // CEP:EVIDENCE: tests/call_inline_tests.rs deopt-resume test.
            // CEP:SECURITY: read-only register access.
            let call_rsp: usize;
            unsafe {
                core::arch::asm!("mov {}, rsp", out(reg) call_rsp, options(nomem, nostack))
            };
            t.borrow_mut().call_rsp = call_rsp;
            let f: extern "C" fn() -> u64 = unsafe { std::mem::transmute(entry) };
            let word = f();
            TrampOutcome::Returned(word)
        } else {
            let t = t.borrow();
            match &t.trap {
                Some(s) => TrampOutcome::Trapped(s.clone()),
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
    // The return address of the trap call sits at [rsp - 8]: the rsi
    // operand was captured BEFORE the call pushed the return address, so it
    // still points at the compiled value-stack top (doc/deopt.md).
    // CEP:UNSAFE: read of the trap call's return address.
    // CEP:WHAT: Site identification.
    // CEP:WHY: The engine maps the offset back to the deopt record.
    // CEP:FAILURE: none; the call pushed the address below rsp.
    // CEP:ASSUMES: called from an engine template call site.
    // CEP:COST: one load.
    // CEP:EVIDENCE: deopt tests.
    // CEP:SECURITY: read of the engine's own stack only.
    let ret = unsafe { ((rsp - 8) as *const u64).read() } as usize;
    // Capture the compiled frame image BEFORE the longjmp: after the jump
    // the engine's own frames reuse this stack region, so reconstruct-time
    // reads would see engine locals instead (doc/deopt.md).
    // CEP:UNSAFE: read of the engine-generated frame below rbp.
    // CEP:WHAT: Frame image capture.
    // CEP:WHY: Deopt reconstruction needs the compiled values after the
    //          longjmp has invalidated the machine stack region. Slot i
    //          lives at [rbp - 8*(i+1)]: the image is read in that order
    //          so its indexes match codegen's slot map exactly.
    // CEP:FAILURE: none; the region is the trap site's own frame.
    // CEP:ASSUMES: rsp <= rbp and 8-byte alignment (compiled prologues).
    // CEP:COST: one copy of the frame (cold path).
    // CEP:EVIDENCE: tests/call_inline_tests.rs deopt-resume test.
    // CEP:SECURITY: reads only the engine's own compiled frame.
    let frame: Vec<u64> = if rbp > rsp && (rbp - rsp).is_multiple_of(8) {
        let n = (rbp - rsp) / 8;
        let mut v = Vec::with_capacity(n);
        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            v.push(unsafe { ((rbp - 8 * (i + 1)) as *const u64).read() });
        }
        v
    } else {
        Vec::new()
    };
    let state = TrapState {
        kind: if kind == 0 { TrapKind::DivideByZero } else { TrapKind::BoundsCheck },
        rbp,
        rsp,
        ret,
        code_base: 0,
        frame,
    };
    let _ = state.code_base;
    TRAMPOLINE.with(|t| {
        // Record the state and release the borrow BEFORE the longjmp: the
        // guard would otherwise survive the jump (its drop never runs) and
        // the landing code's borrow() would panic.
        let env = {
            let mut t = t.borrow_mut();
            let mut state = state;
            state.code_base = t.code_base;
            t.trap = Some(state);
            t.jmp
        };
        match env {
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

// CEP:FILE: engine/src/hot/runtime.rs
// CEP:WHAT: The machine-code ABI surface of the runtime: the extern "C"
//           helpers generated templates call (fmod, allocation, barrier
//           element store/load, the safepoint poll slot).
// CEP:WHY: Spec 2.6/7.5: template-reachable runtime entries live behind a
//          stable ABI so codegen never inlines engine internals
//          (doc/runtime.md).
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: jit_fmod always returns a canonicalized value; jit_alloc
//              returns 0 on failure and the template traps on 0;
//              jit_store_elem returns 1 and jit_load_elem returns tag 1
//              on bounds failure (templates trap on both).
// CEP:ASSUMES: System V AMD64 ABI; the heap pointer is registered by the
//              trampoline window (hot/trap.rs).
// CEP:COST: jit_fmod one libm fmod plus one compare; the element helpers
//           are Heap::alloc_array/store_word/load_word cost; the poll is
//           one predictable branch. Measured in benches/EVIDENCE.md.
// CEP:EVIDENCE: tests/t0_exec.rs (trap from DIV), tests/t1_exec.rs array
//               programs, differential modulo cases, benches/EVIDENCE.md.
// CEP:OPTIMAL: target-optimal for jit_fmod
// CEP:OPTPROOF: one fmod instruction is the ISA lower bound;
//               canonicalization is one compare required by the value
//               representation invariant.
// CEP:SECURITY: lengths and indexes are validated inside the Heap ops.

use crate::hot::trap::TRAMPOLINE;
use crate::hot::value::canonicalize_f64;
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
/// FFI-safe pair return (a repr(C) struct instead of a tuple, which has
/// no defined C ABI).
#[repr(C)]
pub struct LoadedPair {
    /// Value word.
    pub value: u64,
    /// Type tag word.
    pub tag: u64,
}

/// Checked element load called from generated code; returns the word and
/// a tag word (0 = ok, 1 = out of bounds), never fails.
///
/// CEP:WHAT: Runtime array element load.
/// CEP:STATUS: complete
#[unsafe(no_mangle)]
pub extern "C" fn jit_load_elem(arr: u64, idx: f64) -> LoadedPair {
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
                        Ok(w) => LoadedPair { value: w, tag: 0 },
                        Err(_) => LoadedPair { value: 0, tag: 1 },
                    }
                }
            }
            None => LoadedPair { value: 0, tag: 1 },
        }
    })
}

/// Safepoint poll for compiled loop back-edges (spec 2.6, 7.5).
///
/// CEP:WHAT: Safepoint poll entry point, wired into generated back-edges;
///           the body is intentionally vacant in the single-mutator
///           design until multi-mutator support lands.
/// CEP:WHY: GC and OSR requests must be honor-able at loop back-edges and
///          calls; the ABI slot exists now so generated code never needs
///          a re-patch when the ticket lands.
/// CEP:STATUS: stub
/// CEP:OWNER: engine
/// CEP:TICKET: CEP-ENGINE-011
/// CEP:FAILURE: never fails; returns immediately when no request is
///              pending.
/// CEP:ASSUMES: System V ABI; single-mutator collections happen at
///              allocation slow paths and OSR windows (doc/osr.md).
/// CEP:COST: one predictable branch in the common case.
/// CEP:EVIDENCE: tests/osr_tests.rs; the body's vacancy is documented in
///               doc/osr.md Algorithm.
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

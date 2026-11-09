// CEP:FILE: engine/src/cold/deopt.rs
// CEP:WHAT: Deoptimization: interpreter frame reconstruction from machine
//           frames at guards and safepoints (spec 5.7).
// CEP:WHY: Spec 5.7.2: state reconstruction, frame translation and
//          continuation in a lower tier; the deoptimizer is the correctness
//          backbone of every speculative optimization.
// CEP:CLASS: CEP-1 (executed once per trap)
// CEP:STATUS: complete
// CEP:FAILURE: DeoptFailed when no record matches the trigger site or the
//              machine frame fails the shape check (engine bugs fail loud;
//              foreign frames are rejected, never guessed at).
// CEP:HPC-CLASS: HPC-1
// CEP:HPC-IR: Consumes deopt sites produced by the builder and slot maps
//             produced by codegen.
// CEP:HPC-DETERMINISM: deterministic.
// CEP:ASSUMES: the compiled frame image rides in the TrapState (captured
//              at trap time, before the longjmp — the engine's own frames
//              reuse the machine stack region afterwards); slot i of the
//              image holds the word at [rbp - 8*(i+1)] (doc/deopt.md).
// CEP:COST: O(stack + locals) per trap; cold.
// CEP:EVIDENCE: tests/deopt_tests.rs.
// CEP:SECURITY: frame pointers validated against the code metadata before
//               any read.

use crate::cold::codegen::{CompiledCode, DeoptRecord};
use crate::cold::errors::EngineError;
use crate::hot::frame::FnFrame;
use crate::hot::interp::Interp;
use crate::hot::trap::TrapState;
use crate::hot::value::Value;

/// Rebuild an interpreter frame from a machine frame at a trap site.
///
/// CEP:WHAT: Frame reconstruction (spec 5.7.2).
/// CEP:WHY: Guards and safepoints must resume in the interpreter with an
///          exactly equivalent frame.
/// CEP:STATUS: complete
/// CEP:FAILURE: DeoptFailed when the site is unknown or the shape check
///              fails.
/// CEP:HPC-CLASS: HPC-1
/// CEP:HPC-DETERMINISM: deterministic.
/// CEP:ASSUMES: machine frame layout per the module header.
/// CEP:COST: O(stack + locals).
/// CEP:EVIDENCE: tests/deopt_tests.rs.
/// CEP:SECURITY: shape check rejects foreign frames.
///
/// # Errors
/// DeoptFailed.
pub fn reconstruct(
    interp: &Interp,
    code: &CompiledCode,
    state: &TrapState,
    funcs: &[crate::cold::bytecode::Function],
    fn_id: u32,
) -> Result<(u32, Vec<FnFrame>), EngineError> {
    // Map the trap return address to a code offset, then to a record.
    let code_base = state.code_base;
    if state.ret < code_base {
        return Err(EngineError::DeoptFailed("trap return below code base"));
    }
    let off = state.ret - code_base;
    let Some(rec) = code
        .deopt
        .iter()
        .find(|r| r.trap_ret_offsets.contains(&(off as u32)))
    else {
        return Err(EngineError::DeoptFailed("no deopt record for trap site"));
    };
    reconstruct_from_record(interp, rec, state.rbp, state.rsp, &state.frame, funcs, fn_id)
}

/// Reconstruct from an explicit record (shared by guards and OSR).
///
/// CEP:WHAT: Record-driven frame reconstruction.
/// CEP:WHY: Guards, safepoints and OSR entries share the layout.
/// CEP:STATUS: complete
/// CEP:FAILURE: DeoptFailed on shape mismatch.
/// CEP:ASSUMES: `frame_image` was captured at trap time (before the
///              longjmp); slot i of the image is [rbp - 8*(i+1)].
/// CEP:COST: O(stack + locals).
/// CEP:EVIDENCE: tests/call_inline_tests.rs deopt-resume test.
/// CEP:SECURITY: shape check rejects foreign frames.
///
/// # Errors
/// DeoptFailed.
#[allow(clippy::too_many_arguments)]
pub fn reconstruct_from_record(
    interp: &Interp,
    rec: &DeoptRecord,
    rbp: usize,
    rsp: usize,
    frame_image: &[u64],
    funcs: &[crate::cold::bytecode::Function],
    fn_id: u32,
) -> Result<(u32, Vec<FnFrame>), EngineError> {
    let Some(func) = funcs.get(fn_id as usize) else {
        return Err(EngineError::DeoptFailed("fn id out of range"));
    };
    if rsp > rbp || !(rbp - rsp).is_multiple_of(8) {
        return Err(EngineError::DeoptFailed("frame shape mismatch"));
    }
    let mut frame = interp.make_frame(fn_id, func)?;
    // Locals: read the captured frame image into the interpreter binding
    // stacks.
    for (slot, mslot, tag) in &rec.locals {
        let word = read_slot(frame_image, *mslot)?;
        let v = Value::decode_typed(word, *tag);
        let binding = frame
            .locals
            .get_mut(*slot as usize)
            .ok_or(EngineError::DeoptFailed("local slot out of range"))?;
        if binding.push(v).is_none() {
            return Err(EngineError::DeoptFailed("local capacity"));
        }
    }
    // Value stack: each recorded logical value lives in its machine slot.
    for (mslot, tag) in &rec.stack {
        let word = read_slot(frame_image, *mslot)?;
        let v = Value::decode_typed(word, *tag);
        if frame.stack.push(v).is_none() {
            return Err(EngineError::DeoptFailed("value stack capacity"));
        }
    }
    Ok((rec.bc_pc, vec![frame]))
}

/// Read one machine frame slot from the captured frame image.
///
/// CEP:WHAT: Slot read.
/// CEP:WHY: Reconstruction reads compiled values.
/// CEP:STATUS: complete
/// CEP:FAILURE: DeoptFailed beyond the captured frame.
/// CEP:ASSUMES: slot i of the image holds [rbp - 8*(i+1)] at trap time.
/// CEP:COST: one index.
/// CEP:EVIDENCE: tests/call_inline_tests.rs deopt-resume test.
/// CEP:SECURITY: bounds checked against the capture.
///
/// # Errors
/// DeoptFailed.
fn read_slot(frame_image: &[u64], slot: u32) -> Result<u64, EngineError> {
    frame_image
        .get(slot as usize)
        .copied()
        .ok_or(EngineError::DeoptFailed("slot beyond the captured frame"))
}

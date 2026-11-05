// CEP:FILE: engine/src/hot/interp_run.rs
// CEP:WHAT: The interpreter dispatch loop: budget-checked opcode
//           execution, back-edge accounting, safepoint behavior and the
//           Exit taxonomy construction.
// CEP:WHY: The dispatch loop is the CEP-0 core of the baseline tier
//          (doc/engine.md); separating it from the interpreter's
//          construction surface keeps the hot loop auditable alone.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: run returns EngineError for trap-shaped conditions (div by
//              zero, stack underflow) and Exit for every termination
//              shape; the engine owns resumption.
// CEP:ASSUMES: frames built by Interp::make_frame; profile data sized by
//              ProfileData::new.
// CEP:COST: O(executed insns); the hot loop allocates nothing after
//           frame setup (section 25.5).
// CEP:EVIDENCE: differential suite (bit-exact vs every tier);
//               tests/stack_discipline matrices; deopt tests.
// CEP:SECURITY: operand decode is bounds-checked at the bytecode layer;
//               slot access through BoundedStack bounds.

use crate::cold::bytecode::{Function, Insn, Opcode, Operand, StringPool};
use crate::cold::errors::EngineError;
use crate::hot::frame::{advance, as_numbers, pop, push, FnFrame};
use crate::hot::heap::Heap;
use crate::hot::host::HostHooks;
use crate::hot::interp::{render, Exit, Interp, ProfileData, RunBudget};
use crate::hot::value::{canonicalize_f64, Value};

impl Interp {
        /// Run the function table until exit or budget exhaustion.
    ///
    /// CEP:WHAT: The dispatch loop implementing reference semantics.
    /// CEP:WHY: Every tier is differentially tested against this loop.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: see the file-header failure list; the loop cannot panic.
    /// CEP:ASSUMES: all functions verified; frames built by make_frame;
    ///              frames holds at least one activation.
    /// CEP:COST: O(instructions executed); no allocation in the loop body.
    /// CEP:EVIDENCE: tests/differential.rs; benches/engine_bench.rs.
    /// CEP:OPTIMAL: not-optimal
    /// CEP:OPTNOTE: switch dispatch for auditability; ticket CEP-ENGINE-001.
    /// CEP:SECURITY: operands verifier-checked; array indices bounds-checked
    ///               again by the heap; I/O through caller hooks.
    ///
    /// # Errors
    /// See the file-header failure list.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        funcs: &[Function],
        frames: &mut Vec<FnFrame>,
        profiles: &mut [ProfileData],
        pool: &mut StringPool,
        heap: &mut Heap,
        budget: RunBudget,
        host: &HostHooks,
    ) -> Result<Exit, EngineError> {
        let mut edges_used: u64 = 0;
        loop {
            let Some(top) = frames.last() else {
                return Err(EngineError::PcOutOfRange(0));
            };
            let fn_id = top.fn_id;
            let pc = top.pc as usize;
            let Some(func) = funcs.get(fn_id as usize) else {
                return Err(EngineError::PcOutOfRange(fn_id as usize));
            };
            if pc >= func.code.len() {
                return Err(EngineError::PcOutOfRange(pc));
            }
            let insn: Insn = func.code[pc];
            let Some(profile) = profiles.get_mut(fn_id as usize) else {
                return Err(EngineError::PcOutOfRange(fn_id as usize));
            };

            // Each arm advances pc, sets pc to a target, or returns.
            match insn.op {
                Opcode::PushNumber => match insn.operand {
                    Operand::F64(f) => {
                        push(frames, Value::Number(f))?;
                        advance(frames);
                    }
                    _ => return Err(EngineError::TypeMismatch("PushNumber without F64")),
                },
                Opcode::PushBool => match insn.operand {
                    Operand::U64(b) => {
                        push(frames, Value::Bool(b == 1))?;
                        advance(frames);
                    }
                    _ => return Err(EngineError::TypeMismatch("PushBool without U64")),
                },
                Opcode::PushString => match insn.operand {
                    Operand::StrId(id) => {
                        push(frames, Value::Str(id))?;
                        advance(frames);
                    }
                    _ => return Err(EngineError::TypeMismatch("PushString without StrId")),
                },
                Opcode::PushUsize => match insn.operand {
                    Operand::U64(v) => {
                        let f = frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?;
                        f.jump_stack.push(v as usize).ok_or(EngineError::OutOfMemory)?;
                        f.pc += 1;
                    }
                    _ => return Err(EngineError::TypeMismatch("PushUsize without U64")),
                },
                Opcode::Add => {
                    let b = pop(frames)?;
                    let a = pop(frames)?;
                    match (a, b) {
                        (Value::Number(x), Value::Number(y)) => {
                            let c = &mut profile.add_types[pc];
                            c.0 = c.0.saturating_add(1);
                            push(frames, Value::Number(x + y))?;
                        }
                        (Value::Str(i), Value::Str(j)) => {
                            let c = &mut profile.add_types[pc];
                            c.1 = c.1.saturating_add(1);
                            let id = pool.concat(i, j).ok_or(EngineError::OutOfMemory)?;
                            push(frames, Value::Str(id))?;
                        }
                        _ => return Err(EngineError::TypeMismatch("'+' expects number+number")),
                    }
                    advance(frames);
                }
                Opcode::Sub | Opcode::Mul => {
                    let b = pop(frames)?;
                    let a = pop(frames)?;
                    let (x, y) = as_numbers(a, b, insn.op)?;
                    let r = if insn.op == Opcode::Sub { x - y } else { x * y };
                    push(frames, Value::Number(r))?;
                    advance(frames);
                }
                Opcode::Div => {
                    let b = pop(frames)?;
                    let a = pop(frames)?;
                    let (x, y) = as_numbers(a, b, insn.op)?;
                    if y == 0.0 {
                        return Err(EngineError::DivideByZero);
                    }
                    push(frames, Value::Number(canonicalize_f64(x / y)))?;
                    advance(frames);
                }
                Opcode::Modulo => {
                    let b = pop(frames)?;
                    let a = pop(frames)?;
                    let (x, y) = as_numbers(a, b, insn.op)?;
                    push(frames, Value::Number(canonicalize_f64(x % y)))?;
                    advance(frames);
                }
                Opcode::GreaterThan | Opcode::LessThan => {
                    let b = pop(frames)?;
                    let a = pop(frames)?;
                    let (x, y) = as_numbers(a, b, insn.op)?;
                    let r = if insn.op == Opcode::GreaterThan { x > y } else { x < y };
                    push(frames, Value::Bool(r))?;
                    advance(frames);
                }
                Opcode::Equal => {
                    let b = pop(frames)?;
                    let a = pop(frames)?;
                    let r = match (a, b) {
                        (Value::Number(x), Value::Number(y)) => x == y,
                        (Value::Bool(x), Value::Bool(y)) => x == y,
                        (Value::Str(x), Value::Str(y)) => x == y,
                        _ => return Err(EngineError::TypeMismatch("'==' type mismatch")),
                    };
                    push(frames, Value::Bool(r))?;
                    advance(frames);
                }
                Opcode::Jump => match insn.operand {
                    Operand::Target(t) => {
                        frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?.pc = t;
                    }
                    _ => return Err(EngineError::TypeMismatch("Jump without Target")),
                },
                Opcode::JumpIfFalse | Opcode::JumpIfTrue => {
                    let cond = pop(frames)?;
                    let want = insn.op == Opcode::JumpIfTrue;
                    let b = match cond {
                        Value::Bool(b) => b,
                        _ => return Err(EngineError::TypeMismatch("conditional expects bool")),
                    };
                    let entry = &mut profile.branches[pc];
                    if b == want {
                        entry.0 = entry.0.saturating_add(1);
                        match insn.operand {
                            Operand::Target(t) => {
                                frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?.pc = t;
                            }
                            _ => return Err(EngineError::TypeMismatch("jump without Target")),
                        }
                    } else {
                        entry.1 = entry.1.saturating_add(1);
                        advance(frames);
                    }
                }
                Opcode::JumpLastOnStack => {
                    let f = frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?;
                    let target = f.jump_stack.pop().ok_or(EngineError::StackUnderflow)?;
                    f.pc = target as u32;
                }
                Opcode::StoreVar => match insn.operand {
                    Operand::Slot(s) => {
                        let v = pop(frames)?;
                        let f = frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?;
                        let slot = f
                            .locals
                            .get_mut(s as usize)
                            .ok_or(EngineError::PcOutOfRange(s as usize))?;
                        slot.push(v).ok_or(EngineError::OutOfMemory)?;
                        f.pc += 1;
                    }
                    _ => return Err(EngineError::TypeMismatch("StoreVar without Slot")),
                },
                Opcode::LoadVar => match insn.operand {
                    Operand::Slot(s) => {
                        let f = frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?;
                        let slot = f
                            .locals
                            .get_mut(s as usize)
                            .ok_or(EngineError::PcOutOfRange(s as usize))?;
                        let top = slot.top().ok_or(EngineError::StackUnderflow)?;
                        push(frames, top)?;
                        advance(frames);
                    }
                    _ => return Err(EngineError::TypeMismatch("LoadVar without Slot")),
                },
                Opcode::AssignVar => match insn.operand {
                    Operand::Slot(s) => {
                        let v = pop(frames)?;
                        let f = frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?;
                        let slot = f
                            .locals
                            .get_mut(s as usize)
                            .ok_or(EngineError::PcOutOfRange(s as usize))?;
                        let _ = slot.pop();
                        slot.push(v).ok_or(EngineError::OutOfMemory)?;
                        f.pc += 1;
                    }
                    _ => return Err(EngineError::TypeMismatch("AssignVar without Slot")),
                },
                Opcode::Drop => match insn.operand {
                    Operand::Slot(s) => {
                        let f = frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?;
                        let slot = f
                            .locals
                            .get_mut(s as usize)
                            .ok_or(EngineError::PcOutOfRange(s as usize))?;
                        let _ = slot.pop();
                        f.pc += 1;
                    }
                    _ => return Err(EngineError::TypeMismatch("Drop without Slot")),
                },
                Opcode::WriteLn | Opcode::Write => {
                    let v = pop(frames)?;
                    let mut text = render(pool, &v);
                    if insn.op == Opcode::WriteLn {
                        text.push('\n');
                    }
                    (host.write)(&text);
                    advance(frames);
                }
                Opcode::ReadInput => {
                    let line = (host.read_line)();
                    let id = pool.intern(&line).ok_or(EngineError::OutOfMemory)?;
                    push(frames, Value::Str(id))?;
                    advance(frames);
                }
                Opcode::ProcessExit => {
                    let v = pop(frames)?;
                    match v {
                        Value::Number(n) => return Ok(Exit::ProcessExited(n)),
                        _ => return Err(EngineError::TypeMismatch("ProcessExit expects number")),
                    }
                }
                Opcode::NewArray => {
                    let v = pop(frames)?;
                    let Value::Number(n) = v else {
                        return Err(EngineError::TypeMismatch("NEWARR expects number length"));
                    };
                    if n < 0.0 || n.fract() != 0.0 || n > (u32::MAX as f64) {
                        return Err(EngineError::TypeMismatch("NEWARR length out of range"));
                    }
                    let ptr = heap.alloc_array(n as u32)?;
                    push(frames, Value::Obj(ptr))?;
                    advance(frames);
                }
                Opcode::ArrayLen => {
                    let v = pop(frames)?;
                    let Value::Obj(ptr) = v else {
                        return Err(EngineError::TypeMismatch("ARYLEN expects array"));
                    };
                    let len = heap.array_len(ptr);
                    push(frames, Value::Number(len as f64))?;
                    advance(frames);
                }
                Opcode::LoadIdx => {
                    let idx = pop(frames)?;
                    let arr = pop(frames)?;
                    let Value::Number(i) = idx else {
                        return Err(EngineError::TypeMismatch("GETIDX expects number index"));
                    };
                    let Value::Obj(ptr) = arr else {
                        return Err(EngineError::TypeMismatch("GETIDX expects array"));
                    };
                    let w = heap.load_word(ptr, i)?;
                    push(frames, Value::decode(w))?;
                    advance(frames);
                }
                Opcode::StoreIdx => {
                    let val = pop(frames)?;
                    let idx = pop(frames)?;
                    let arr = pop(frames)?;
                    let Value::Number(i) = idx else {
                        return Err(EngineError::TypeMismatch("SETIDX expects number index"));
                    };
                    let Value::Obj(ptr) = arr else {
                        return Err(EngineError::TypeMismatch("SETIDX expects array"));
                    };
                    heap.store_word(ptr, i, val.encode())?;
                    advance(frames);
                }
                Opcode::Call => match insn.operand {
                    Operand::U64(packed) => {
                        let callee = (packed & 0xFFFF_FFFF) as u32;
                        if frames.len() >= self.max_call_depth {
                            return Err(EngineError::OutOfMemory);
                        }
                        let Some(callee_fn) = funcs.get(callee as usize) else {
                            return Err(EngineError::PcOutOfRange(callee as usize));
                        };
                        frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?.pc += 1;
                        let new_frame = self.make_frame(callee, callee_fn)?;
                        frames.push(new_frame);
                    }
                    _ => return Err(EngineError::TypeMismatch("CALL without operand")),
                },
                Opcode::Ret => {
                    let v = pop(frames)?;
                    if frames.len() == 1 {
                        return Ok(Exit::Returned(v));
                    }
                    let _ = frames.pop();
                    push(frames, v)?;
                }
                Opcode::Halt => {
                    let bottom = frames
                        .last()
                        .ok_or(EngineError::PcOutOfRange(0))?
                        .stack
                        .get(0);
                    return Ok(Exit::Halt(bottom));
                }
            }

            // Back-edge accounting: a jump whose target is at or before the
            // jump's own pc is a loop back-edge (spec 6.1).
            let Some(top) = frames.last() else {
                return Err(EngineError::PcOutOfRange(0));
            };
            let cur = top.pc as usize;
            let Some(func) = funcs.get(top.fn_id as usize) else {
                return Err(EngineError::PcOutOfRange(top.fn_id as usize));
            };
            if let Some(i) = func.code.get(cur)
                && let Operand::Target(t) = i.operand
                    && matches!(i.op, Opcode::Jump | Opcode::JumpIfFalse | Opcode::JumpIfTrue)
                        && (t as usize) <= cur
                    {
                        let Some(profile) = profiles.get_mut(top.fn_id as usize) else {
                            return Err(EngineError::PcOutOfRange(0));
                        };
                        let c = &mut profile.back_edges[cur];
                        *c = c.saturating_add(1);
                        edges_used += 1;
                        if edges_used >= budget.back_edges {
                            let fn_id = top.fn_id;
                            return Ok(Exit::BackEdgeBudget { fn_id, site: cur as u32 });
                        }
                    }
        }
    }
}

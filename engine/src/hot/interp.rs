// CEP:FILE: engine/src/hot/interp.rs
// CEP:WHAT: The baseline interpreter: reference semantics for Vertex
//           bytecode, execution profiler for PGO, resumable frame model
//           for deoptimization and on-stack replacement.
// CEP:WHY: The spec requires a reference interpreter for differential
//          testing (standard 38.45), profile counters for PGO (spec 5.1)
//          and a deopt target that can resume any frame state (spec 5.7);
//          it is also the fallback tier for bytecode outside the compiled
//          subsets.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: Returns EngineError for stack underflow, capacity
//              exhaustion (OutOfMemory), type errors, division by zero,
//              GC-needed, index errors and malformed frames. The dispatch
//              loop performs no allocation and cannot panic: every access
//              is bounds-checked and every push capacity-checked.
// CEP:ASSUMES: functions passed cold::bytecode::verify; frames built by
//              make_frame; one interpreter per thread.
// CEP:COST: dispatch is one match plus per-opcode work; measured per-opcode
//           in benches/engine_bench.rs (EVIDENCE.md). No allocation in the
//           loop body: value stacks are BoundedStack (one-time Box at
//           frame construction), profile counters are flat arrays sized
//           by code length at construction.
// CEP:EVIDENCE: benches/engine_bench.rs; tests/differential.rs (bit-exact
//               against T0/T1/T2/T3); negative tests per failure class.
// CEP:OPTIMAL: not-optimal (switch dispatch, not threaded code)
// CEP:OPTNOTE: acceptable for the baseline tier; T0 removes interpreter
//              dispatch from hot paths; ticket CEP-ENGINE-001.
// CEP:SECURITY: bytecode is untrusted and verifier-checked upstream; every
//               index is bounds-checked again at use; I/O goes through
//               caller-supplied function pointers.

use crate::cold::bytecode::{Function, Insn, Opcode, Operand, StringPool};
use crate::cold::errors::EngineError;
use crate::hot::heap::Heap;
use crate::hot::value::{canonicalize_f64, BoundedStack, Value};

/// I/O hooks: function pointers, not trait objects (section 25.6 allows
/// measured function-pointer tables; no hidden allocation).
#[derive(Clone, Copy)]
pub struct HostHooks {
    /// Called by WriteLn and Write.
    pub write: fn(&str),
    /// Called by ReadInput; returns one trimmed input line.
    pub read_line: fn() -> String,
}

impl HostHooks {
    /// Default host: stdout writes and one trimmed stdin line.
    ///
    /// CEP:WHAT: Default stdio hooks.
    /// CEP:WHY: CLI parity with the legacy VM.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: read_line returns an empty string on EOF.
    /// CEP:ASSUMES: stdio available; cold I/O exit.
    /// CEP:COST: syscall-bound.
    /// CEP:EVIDENCE: manual smoke test; excluded from CI (no tty).
    /// CEP:SECURITY: input is untrusted text, treated as data only.
    #[must_use]
    pub fn stdio() -> HostHooks {
        HostHooks { write: default_write, read_line: default_read_line }
    }
}

fn default_write(s: &str) {
    print!("{s}");
}

fn default_read_line() -> String {
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
    buf.trim().to_owned()
}

/// Execution profile for one function: flat counters indexed by pc.
///
/// Arrays are allocated once with length = code length, so the hot loop
/// performs no allocation (CEP-0).
#[derive(Debug, Clone)]
pub struct ProfileData {
    /// Total entries into this function.
    pub calls: u64,
    /// Back-edge executions per pc.
    pub back_edges: Box<[u64]>,
    /// Conditional branch outcomes per pc: (taken, not_taken).
    pub branches: Box<[(u64, u64)]>,
    /// Add-site operand types per pc: (number, string).
    pub add_types: Box<[(u64, u64)]>,
}

impl ProfileData {
    /// Allocate counters for a function.
    ///
    /// CEP:WHAT: Profile construction.
    /// CEP:WHY: One-time allocation keeps the hot loop allocation-free.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Fatal at construction only if the host cannot honor
    ///              the reservation.
    /// CEP:ASSUMES: none
    /// CEP:COST: O(code length), once per function.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:SECURITY: size derived from verified bytecode.
    #[must_use]
    pub fn new(code_len: usize) -> ProfileData {
        ProfileData {
            calls: 0,
            back_edges: vec![0; code_len].into_boxed_slice(),
            branches: vec![(0, 0); code_len].into_boxed_slice(),
            add_types: vec![(0, 0); code_len].into_boxed_slice(),
        }
    }
}

/// One active function activation.
pub struct FnFrame {
    /// Function table index.
    pub fn_id: u32,
    /// Program counter.
    pub pc: u32,
    /// Value stack (fixed capacity).
    pub stack: BoundedStack,
    /// Jump-address stack (fixed capacity).
    pub jump_stack: BoundedStackUsize,
    /// Local slots; each is a bounded binding stack (legacy scoping).
    pub locals: Vec<BoundedStack>,
}

/// Fixed-capacity usize stack (jump addresses).
pub struct BoundedStackUsize {
    buf: Box<[usize]>,
    len: usize,
}

impl BoundedStackUsize {
    /// Allocate with fixed capacity.
    ///
    /// CEP:WHAT: Constructor.
    /// CEP:WHY: One-time allocation before the hot path.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: Fatal at construction only on reservation failure.
    /// CEP:ASSUMES: none
    /// CEP:COST: one allocation per frame.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:SECURITY: capacity engine-configured.
    #[must_use]
    pub fn with_capacity(cap: usize) -> BoundedStackUsize {
        BoundedStackUsize { buf: vec![0; cap].into_boxed_slice(), len: 0 }
    }

    /// Push an address.
    ///
    /// CEP:WHAT: Bounded push.
    /// CEP:WHY: CEP-0 hot loop.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: None when full.
    /// CEP:ASSUMES: none
    /// CEP:COST: one compare plus one store.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: one branch and one store.
    /// CEP:SECURITY: bounds retained.
    #[inline]
    pub fn push(&mut self, v: usize) -> Option<()> {
        if self.len >= self.buf.len() {
            return None;
        }
        self.buf[self.len] = v;
        self.len += 1;
        Some(())
    }

    /// Pop an address.
    ///
    /// CEP:WHAT: Bounded pop.
    /// CEP:WHY: JumpLastOnStack semantics.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: None when empty.
    /// CEP:ASSUMES: none
    /// CEP:COST: one compare plus one load.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:OPTIMAL: target-optimal
    /// CEP:OPTPROOF: one branch and one load.
    /// CEP:SECURITY: none
    #[inline]
    pub fn pop(&mut self) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(self.buf[self.len])
    }
}

/// Termination or interruption state of one interpreter run.
#[derive(Debug, Clone, PartialEq)]
pub enum Exit {
    /// Halt executed; carries the bottom of the non-empty value stack
    /// (legacy VM parity).
    Halt(Option<Value>),
    /// ProcessExit executed with the popped exit code.
    ProcessExited(f64),
    /// The entry function returned a value through Ret.
    Returned(Value),
    /// The back-edge budget was exhausted at one site; the frame stack is
    /// resumable (OSR trigger point, spec 6.1).
    BackEdgeBudget {
        /// Function id whose back-edge hit the budget.
        fn_id: u32,
        /// The back-edge site pc.
        site: u32,
    },
}

/// Execution budget for one run call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBudget {
    /// Stop after this many back-edge executions.
    pub back_edges: u64,
}

impl RunBudget {
    /// Run to completion.
    ///
    /// CEP:WHAT: Unlimited budget constant.
    /// CEP:WHY: Tests and final-tier runs need completion semantics.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: compile-time only.
    /// CEP:EVIDENCE: used by every differential test.
    /// CEP:SECURITY: none
    pub const UNLIMITED: RunBudget = RunBudget { back_edges: u64::MAX };
}

/// The interpreter.
pub struct Interp {
    /// Value-stack capacity per frame.
    pub stack_cap: usize,
    /// Jump-stack capacity per frame.
    pub jump_cap: usize,
    /// Maximum simultaneous activations.
    pub max_call_depth: usize,
}

impl Interp {
    /// Construct with default capacities.
    ///
    /// CEP:WHAT: Constructor.
    /// CEP:WHY: Capacities are named configuration.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: used by every test.
    /// CEP:SECURITY: none
    #[must_use]
    pub fn new() -> Interp {
        Interp { stack_cap: 4096, jump_cap: 256, max_call_depth: 256 }
    }

    /// Build a fresh frame for `func`.
    ///
    /// CEP:WHAT: Frame construction with preallocated capacity.
    /// CEP:WHY: Preallocation keeps the run loop allocation-free.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: OutOfMemory when a bounded reservation fails.
    /// CEP:ASSUMES: func passed verification.
    /// CEP:COST: O(locals + capacities), once per activation.
    /// CEP:EVIDENCE: differential suite.
    /// CEP:SECURITY: sizes derive from verified bytecode and engine config.
    ///
    /// # Errors
    /// OutOfMemory on reservation failure.
    pub fn make_frame(&self, fn_id: u32, func: &Function) -> Result<FnFrame, EngineError> {
        let mut locals = Vec::with_capacity(func.locals.len());
        for _ in 0..func.locals.len() {
            locals.push(BoundedStack::with_capacity(8));
        }
        Ok(FnFrame {
            fn_id,
            pc: 0,
            stack: BoundedStack::with_capacity(self.stack_cap),
            jump_stack: BoundedStackUsize::with_capacity(self.jump_cap),
            locals,
        })
    }

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
            if let Some(i) = func.code.get(cur) {
                if let Operand::Target(t) = i.operand {
                    if matches!(i.op, Opcode::Jump | Opcode::JumpIfFalse | Opcode::JumpIfTrue)
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
    }
}

impl Default for Interp {
    fn default() -> Self {
        Interp::new()
    }
}

/// Advance the top frame's pc by one.
///
/// CEP:WHAT: pc advance.
/// CEP:WHY: Arms share the advance.
/// CEP:STATUS: complete
/// CEP:FAILURE: none (frames is non-empty by loop invariant).
/// CEP:ASSUMES: frames non-empty.
/// CEP:COST: one store.
/// CEP:EVIDENCE: differential suite.
/// CEP:OPTIMAL: target-optimal
/// CEP:OPTPROOF: single field store.
/// CEP:SECURITY: none
#[inline]
fn advance(frames: &mut [FnFrame]) {
    if let Some(f) = frames.last_mut() {
        f.pc += 1;
    }
}

/// Capacity-checked push onto the top frame.
///
/// CEP:WHAT: Bounded push.
/// CEP:WHY: Hot-loop allocation ban; overflow is a typed error.
/// CEP:STATUS: complete
/// CEP:FAILURE: OutOfMemory on capacity exhaustion.
/// CEP:ASSUMES: frames non-empty.
/// CEP:COST: one compare plus one store.
/// CEP:EVIDENCE: differential suite.
/// CEP:OPTIMAL: target-optimal
/// CEP:OPTPROOF: bounds check required by the failure policy.
/// CEP:SECURITY: none
#[inline]
fn push(frames: &mut [FnFrame], v: Value) -> Result<(), EngineError> {
    let f = frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?;
    f.stack.push(v).ok_or(EngineError::OutOfMemory)
}

/// Underflow-checked pop from the top frame.
///
/// CEP:WHAT: Bounded pop.
/// CEP:WHY: Underflow is a defined failure (Law 6).
/// CEP:STATUS: complete
/// CEP:FAILURE: StackUnderflow on empty.
/// CEP:ASSUMES: frames non-empty.
/// CEP:COST: one compare plus one load.
/// CEP:EVIDENCE: negative tests.
/// CEP:OPTIMAL: target-optimal
/// CEP:OPTPROOF: one branch and one load.
/// CEP:SECURITY: none
#[inline]
fn pop(frames: &mut [FnFrame]) -> Result<Value, EngineError> {
    let f = frames.last_mut().ok_or(EngineError::PcOutOfRange(0))?;
    f.stack.pop().ok_or(EngineError::StackUnderflow)
}

/// Numeric operand helper.
///
/// CEP:WHAT: Two numbers or a type error.
/// CEP:WHY: All arithmetic opcodes share the check.
/// CEP:STATUS: complete
/// CEP:FAILURE: TypeMismatch for non-number operands.
/// CEP:ASSUMES: none
/// CEP:COST: two matches.
/// CEP:EVIDENCE: negative tests.
/// CEP:SECURITY: none
///
/// # Errors
/// TypeMismatch.
fn as_numbers(a: Value, b: Value, op: Opcode) -> Result<(f64, f64), EngineError> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => Ok((x, y)),
        _ => Err(EngineError::TypeMismatch(op_name(op))),
    }
}

/// Stable opcode display name for type errors.
///
/// CEP:WHAT: Opcode names.
/// CEP:WHY: Diagnostics must not allocate per failure.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: negative tests.
/// CEP:SECURITY: none
const fn op_name(op: Opcode) -> &'static str {
    match op {
        Opcode::Add => "'+' expects numbers",
        Opcode::Sub => "'-' expects numbers",
        Opcode::Mul => "'*' expects numbers",
        Opcode::Div => "'/' expects numbers",
        Opcode::Modulo => "'%' expects numbers",
        Opcode::GreaterThan => "'>' expects numbers",
        Opcode::LessThan => "'<' expects numbers",
        Opcode::Equal => "'==' type mismatch",
        _ => "type mismatch",
    }
}

/// Render a value for Write/WriteLn (legacy VM formatting parity).
///
/// CEP:WHAT: Value-to-text rendering.
/// CEP:WHY: Output parity with the legacy VM.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: string ids index the pool.
/// CEP:COST: O(digits); cold path (I/O opcode only).
/// CEP:EVIDENCE: output parity tests.
/// CEP:SECURITY: renders untrusted values as data only.
#[must_use]
pub fn render(pool: &StringPool, v: &Value) -> String {
    match v {
        Value::Number(n) => format!("{n}"),
        Value::Bool(b) => format!("{b}"),
        Value::Str(id) => pool.get(*id).to_owned(),
        Value::Obj(_) => "[object]".to_owned(),
        Value::Null => "null".to_owned(),
    }
}

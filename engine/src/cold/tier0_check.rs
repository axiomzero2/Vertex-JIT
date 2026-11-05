// CEP:FILE: engine/src/cold/tier0_check.rs
// CEP:WHAT: The T0 static legality checker: the type lattice, the
//           fixed-capacity abstract state, the worklist landing rule and
//           the public check_legality gate.
// CEP:WHY: T0 accepts only programs it can prove safe; the checker is the
//          T0 trust gate for untrusted bytecode (doc/tier0.md, spec 2.1).
// CEP:CLASS: CEP-1 (compilation); emitted code is CEP-0.
// CEP:STATUS: complete
// CEP:FAILURE: check_legality returns IllegalForT0 for every program
//              outside the T0 subset; the caller falls back to the
//              interpreter (never silently). NotConvergent stays as a
//              defensive fail-closed bound.
// CEP:ASSUMES: verified bytecode shape; each pc is enqueued at most once
//              under equality joins (single-mutation worklist).
// CEP:COST: O(insns): each pc is enqueued once and visited once; states
//           move in place, cloned only at two-arm branches.
// CEP:EVIDENCE: legality positive/negative tests; tier0.rs tests.
// CEP:SECURITY: this checker is the T0 trust gate for untrusted bytecode.

use std::collections::VecDeque;

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

/// Fixed-capacity abstract value stack. State clones must stay
/// allocation-free to hold the spec 2.1 T0 compile-latency budget, so the
/// stack lives in inline cells sized by the documented limits; `len`
/// saturates instead of panicking and the depth checks above it fail
/// closed.
#[derive(Debug, Clone, Copy)]
struct FixedStack {
    cells: [Lat; MAX_T0_STACK_DEPTH + 1],
    len: u8,
}

impl FixedStack {
    /// All-Num initial abstract stack.
/// CEP:WHAT: Abstract-stack construction.
/// CEP:WHY: Every pc starts with the default
///              lattice state; fixed capacity means no allocation.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: legality tests.
    fn new() -> Self {
        Self {
            cells: [Lat::Num; MAX_T0_STACK_DEPTH + 1],
            len: 0,
        }
    }
    fn len(&self) -> usize {
        self.len as usize
    }
    fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Push one lattice cell (fail-closed at capacity).
/// CEP:WHAT: Lattice push with a silent-at-capacity clamp that
///              land_state rejects via the depth check.
/// CEP:WHY: The abstract stack is fixed-capacity (MAX_T0_STACK_DEPTH);
///              overflow is caught by the depth check, not by
///              allocation.
/// CEP:STATUS: complete
/// CEP:FAILURE: pushes beyond capacity are dropped here and rejected
///              by the depth check upstream.
/// CEP:ASSUMES: capacity is MAX_T0_STACK_DEPTH + 1.
/// CEP:COST: one store plus compare.
/// CEP:EVIDENCE: legality tests cover the depth boundary.
    fn push(&mut self, lat: Lat) {
        let i = self.len as usize;
        if i < self.cells.len() {
            self.cells[i] = lat;
        }
        self.len = self.len.saturating_add(1);
    }
    /// Pop one lattice cell.
/// CEP:WHAT: Lattice pop at the abstract-stack top.
/// CEP:WHY: compare templates consume two operands and push one result;
///              the lattice mirrors that discipline.
/// CEP:STATUS: complete
/// CEP:FAILURE: None at the bottom (callers treat underflow as
///              IllegalForT0).
/// CEP:ASSUMES: push/pop pairs are balanced per template.
/// CEP:COST: one load plus compare.
/// CEP:EVIDENCE: legality tests cover underflow.
    fn pop(&mut self) -> Option<Lat> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(self.cells[self.len as usize])
    }
    fn last(&self) -> Option<&Lat> {
        if self.len == 0 {
            None
        } else {
            Some(&self.cells[self.len as usize - 1])
        }
    }
    /// Truncate the abstract stack to a depth (branch joins).
/// CEP:WHAT: Depth truncate for control-flow joins.
/// CEP:WHY: A branch successor's state must match the join depth exactly;
///              truncate models pops the templates performed.
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: n <= current len (callers guarantee).
/// CEP:COST: O(1).
/// CEP:EVIDENCE: legality tests cover join shapes.
    fn truncate(&mut self, n: usize) {
        if n < self.len as usize {
            self.len = n as u8;
        }
    }
}

/// Equality joins compare only live cells: pop/truncate leave stale bytes
/// beyond `len`, and comparing them would manufacture merge conflicts.
impl PartialEq for FixedStack {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self.cells[..self.len as usize] == other.cells[..other.len as usize]
    }
}
impl Eq for FixedStack {}

/// Abstract state at one pc. Fully inline and Copy: a visit clones by a
/// 160-byte memcpy with no allocator traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct State {
    stack: FixedStack,
    /// Tracked lattice type per slot; entries >= locals.len() stay Num.
    slot_types: [Lat; MAX_T0_LOCALS],
    /// Live-binding count per slot (scope model, doc/tier0.md); entries
    /// at or above locals.len() stay 0 and the slot bound check below
    /// rejects any operand that reaches them.
    binding: [u8; MAX_T0_LOCALS],
}

/// Land one successor edge: fail closed on out-of-range successors and
/// over-deep stacks, record the depth, store the state on first arrival
/// (enqueuing the pc), and enforce the equality join on re-arrival.
/// CEP:WHAT: Join one abstract state into the worklist at a pc.
/// CEP:WHY: The single-mutation worklist: under equality joins a pc's state
///              lands at most once, so recomputation is provably idle
///              (the O(insns) latency bound).
/// CEP:STATUS: complete
/// CEP:FAILURE: Err (NotConvergent) as the defensive fail-closed bound;
///              IllegalForT0 on depth violations.
/// CEP:ASSUMES: states are fixed-capacity Copy values; joins are
///              equality-based.
/// CEP:COST: O(1) amortized per landing.
/// CEP:EVIDENCE: legality tests; the 5 us latency budget in release CI.
pub(crate) fn land_state(
    states: &mut [Option<State>],
    queue: &mut VecDeque<usize>,
    max_stack: &mut usize,
    len: usize,
    succ: usize,
    s: State,
) -> Result<(), IllegalForT0> {
    if succ >= len {
        return Err(IllegalForT0::TypeMismatch("successor out of range"));
    }
    if s.stack.len() > MAX_T0_STACK_DEPTH {
        return Err(IllegalForT0::StackTooDeep(s.stack.len()));
    }
    *max_stack = (*max_stack).max(s.stack.len());
    match &states[succ] {
        None => {
            states[succ] = Some(s);
            queue.push_back(succ);
        }
        Some(existing) => {
            if *existing != s {
                return Err(IllegalForT0::TypeMismatch("state conflict at merge point"));
            }
        }
    }
    Ok(())
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
/// CEP:COST: O(insns): each pc is enqueued once and visited once; states
///              move in place, cloned only at two-arm branches.
/// CEP:EVIDENCE: legality positive/negative tests.
/// CEP:SECURITY: this checker is the T0 trust gate for untrusted bytecode.
pub fn check_legality(
    func: &crate::cold::bytecode::Function,
) -> Result<LegalityReport, IllegalForT0> {
    use crate::cold::bytecode::{Opcode, Operand};
    let n_locals = func.locals.len();
    if n_locals > MAX_T0_LOCALS {
        return Err(IllegalForT0::TooManyLocals(n_locals));
    }
    let len = func.code.len();
    let mut states: Vec<Option<State>> = vec![None; len];
    states[0] = Some(State {
        stack: FixedStack::new(),
        slot_types: [Lat::Num; MAX_T0_LOCALS],
        binding: [0; MAX_T0_LOCALS],
    });
    let mut max_stack: usize = 0;
    // Worklist, not pass-loop. Under equality joins a pc's state lands at
    // most once: a later different store is a merge conflict, a later equal
    // store is a no-op. Recomputing a visited pc is therefore provably idle
    // work whose only effect is re-cloning states -- the pass-loop paid
    // O(n^2) deep clones (3 Vecs each) and crossed the T0 compile-latency
    // budget on 64-insn programs. Each pc is enqueued exactly when its state
    // is stored and visited exactly once, and states stay in place so every
    // later edge still sees them for the merge comparison; a visit clones
    // its state once and mutates the clone, twice only where two successors
    // share one state (conditional branch).
    let mut queue: VecDeque<usize> = VecDeque::with_capacity(len);
    queue.push_back(0);
    let mut visits = 0usize;
    while let Some(pc) = queue.pop_front() {
        visits += 1;
        if visits > len {
            // Defensive fail-closed: unreachable, because every enqueue
            // follows the single None->Some store of one pc.
            return Err(IllegalForT0::NotConvergent);
        }
        let mut state = states[pc].expect("a pc is enqueued only after its state is stored");
        if state.stack.len() > MAX_T0_STACK_DEPTH {
            return Err(IllegalForT0::StackTooDeep(state.stack.len()));
        }
        max_stack = max_stack.max(state.stack.len());
        let insn = func.code[pc];
        match insn.op {
            Opcode::PushNumber | Opcode::PushBool => {
                state.stack.push(Lat::Num);
                land_state(&mut states, &mut queue, &mut max_stack, len, pc + 1, state)?;
            }
            Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div | Opcode::Modulo
            | Opcode::GreaterThan | Opcode::LessThan | Opcode::Equal => {
                if state.stack.len() < 2 {
                    return Err(IllegalForT0::TypeMismatch("binary op needs two operands"));
                }
                let out = match insn.op {
                    Opcode::GreaterThan | Opcode::LessThan | Opcode::Equal => Lat::Bool,
                    _ => Lat::Num,
                };
                state.stack.truncate(state.stack.len() - 2);
                state.stack.push(out);
                land_state(&mut states, &mut queue, &mut max_stack, len, pc + 1, state)?;
            }
            Opcode::JumpIfFalse | Opcode::JumpIfTrue => {
                if state.stack.last() != Some(&Lat::Bool) {
                    return Err(IllegalForT0::TypeMismatch("condition must be bool"));
                }
                let Operand::Target(t) = insn.operand else {
                    return Err(IllegalForT0::TypeMismatch("jump without target"));
                };
                state.stack.pop();
                let fall = state;
                land_state(&mut states, &mut queue, &mut max_stack, len, t as usize, state)?;
                land_state(&mut states, &mut queue, &mut max_stack, len, pc + 1, fall)?;
            }
            Opcode::Jump => {
                let Operand::Target(t) = insn.operand else {
                    return Err(IllegalForT0::TypeMismatch("jump without target"));
                };
                land_state(&mut states, &mut queue, &mut max_stack, len, t as usize, state)?;
            }
            Opcode::LoadVar => {
                let Operand::Slot(sl) = insn.operand else {
                    return Err(IllegalForT0::TypeMismatch("LoadVar without slot"));
                };
                if sl as usize >= n_locals {
                    return Err(IllegalForT0::TypeMismatch("slot operand out of range"));
                }
                if state.binding[sl as usize] < 1 {
                    return Err(IllegalForT0::TypeMismatch("load of unbound variable"));
                }
                let lat = state.slot_types[sl as usize];
                state.stack.push(lat);
                land_state(&mut states, &mut queue, &mut max_stack, len, pc + 1, state)?;
            }
            Opcode::StoreVar => {
                let Operand::Slot(sl) = insn.operand else {
                    return Err(IllegalForT0::TypeMismatch("StoreVar without slot"));
                };
                if sl as usize >= n_locals {
                    return Err(IllegalForT0::TypeMismatch("slot operand out of range"));
                }
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
                let v = state.stack.pop().expect("checked non-empty above");
                state.slot_types[sl as usize] = v;
                state.binding[sl as usize] = 1;
                land_state(&mut states, &mut queue, &mut max_stack, len, pc + 1, state)?;
            }
            Opcode::AssignVar => {
                let Operand::Slot(sl) = insn.operand else {
                    return Err(IllegalForT0::TypeMismatch("AssignVar without slot"));
                };
                if sl as usize >= n_locals {
                    return Err(IllegalForT0::TypeMismatch("slot operand out of range"));
                }
                if state.stack.is_empty() {
                    return Err(IllegalForT0::TypeMismatch("assign on empty stack"));
                }
                if state.binding[sl as usize] < 1 {
                    return Err(IllegalForT0::TypeMismatch("assign of unbound variable"));
                }
                let v = state.stack.pop().expect("checked non-empty above");
                if state.slot_types[sl as usize] != v {
                    return Err(IllegalForT0::TypeMismatch(
                        "assign changes the slot lattice type",
                    ));
                }
                land_state(&mut states, &mut queue, &mut max_stack, len, pc + 1, state)?;
            }
            Opcode::Drop => {
                let Operand::Slot(sl) = insn.operand else {
                    return Err(IllegalForT0::TypeMismatch("Drop without slot"));
                };
                if sl as usize >= n_locals {
                    return Err(IllegalForT0::TypeMismatch("slot operand out of range"));
                }
                if state.binding[sl as usize] < 1 {
                    return Err(IllegalForT0::TypeMismatch("drop of unbound variable"));
                }
                state.binding[sl as usize] -= 1;
                land_state(&mut states, &mut queue, &mut max_stack, len, pc + 1, state)?;
            }
            Opcode::Halt => {}
            other => return Err(IllegalForT0::Opcode(other)),
        }
    }
    Ok(LegalityReport { max_stack: max_stack as u32 })
}

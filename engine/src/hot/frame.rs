// CEP:FILE: engine/src/hot/frame.rs
// CEP:WHAT: Interpreter frame machinery: the call frame, the fixed-capacity
//           slot stack, and the frame-stack primitives (advance, push,
//           pop, numeric operand decoding).
// CEP:WHY: Frames carry the deopt metadata and the value slots; keeping
//          the machinery separate from the dispatch loop keeps both
//          auditable (doc/deopt.md, doc/engine.md).
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: push/pop return the defined boundary errors; as_numbers
//              returns TypeMismatch for non-numeric operands.
// CEP:ASSUMES: frames are engine-constructed; slots hold engine values.
// CEP:COST: push/pop are branch plus access; as_numbers decodes in place.
// CEP:EVIDENCE: differential suite; deopt tests.
// CEP:SECURITY: slot access is bounds-checked at the BoundedStack layer.

use crate::cold::errors::EngineError;
use crate::hot::value::Value;
use crate::cold::bytecode::Opcode;
use crate::hot::stack::BoundedStack;

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

/// Advance the frame pointer of the last frame (dispatch helper).
/// CEP:WHAT: Frame pc advance after each executed instruction.
/// CEP:WHY: The dispatch loop moves every active frame's pc forward on
///              function exit paths (unwinding to the caller).
/// CEP:STATUS: complete
/// CEP:FAILURE: none (index arithmetic over engine frames).
/// CEP:ASSUMES: frames is non-empty when called.
/// CEP:COST: O(1).
/// CEP:EVIDENCE: differential suite; deopt tests.
pub(crate) fn advance(frames: &mut [FnFrame]) {
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
pub(crate) fn push(frames: &mut [FnFrame], v: Value) -> Result<(), EngineError> {
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
pub(crate) fn pop(frames: &mut [FnFrame]) -> Result<Value, EngineError> {
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
pub(crate) fn as_numbers(a: Value, b: Value, op: Opcode) -> Result<(f64, f64), EngineError> {
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

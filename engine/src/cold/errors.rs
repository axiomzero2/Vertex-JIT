// CEP:FILE: engine/src/cold/errors.rs
// CEP:WHAT: The engine error taxonomy shared by all modules.
// CEP:WHY: Law 6 requires every failure to be an explicit, named value;
//          one enum keeps the failure surface auditable.
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: This type is the failure surface; see variant docs.
// CEP:ASSUMES: none
// CEP:COST: constructed on failure paths only.
// CEP:EVIDENCE: review of all error construction sites.
// CEP:SECURITY: error text never embeds untrusted input verbatim.

use core::fmt;

/// Engine-wide error type.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// Bytecode failed to decode or verify.
    Bytecode(crate::cold::bytecode::BytecodeError),
    /// The host platform does not satisfy the target contract.
    Target(crate::platform::TargetError),
    /// The code cache budget is exhausted (38.39).
    CodeCacheFull {
        /// Budget in bytes.
        budget: u32,
        /// Bytes requested.
        requested: u32,
    },
    /// A compilation unit exceeded the IR node budget.
    IrTooLarge {
        /// Budget in nodes.
        budget: u32,
    },
    /// The IR verifier rejected the graph (38.18).
    IrVerificationFailed(String),
    /// Runtime type error (mirrors the legacy VM).
    TypeMismatch(&'static str),
    /// Division or modulo by zero.
    DivideByZero,
    /// Array index outside object bounds.
    IndexOutOfBounds(f64),
    /// Interpreter executed an out-of-range pc (invariant failure).
    PcOutOfRange(usize),
    /// Value stack underflow.
    StackUnderflow,
    /// A guard failed and deoptimization could not reconstruct the frame.
    DeoptFailed(&'static str),
    /// OSR entry requested for a non-loop-header pc.
    OsrBadEntry(usize),
    /// Heap allocation failed or budget exhausted.
    OutOfMemory,
    /// The heap is full; a collection is required before retrying.
    GcNeeded,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Bytecode(e) => write!(f, "bytecode error: {e}"),
            EngineError::Target(e) => write!(f, "target error: {e}"),
            EngineError::CodeCacheFull { budget, requested } => {
                write!(f, "code cache full: budget {budget}, requested {requested}")
            }
            EngineError::IrTooLarge { budget } => write!(f, "IR exceeds {budget} nodes"),
            EngineError::IrVerificationFailed(m) => write!(f, "IR verification failed: {m}"),
            EngineError::TypeMismatch(m) => write!(f, "type error: {m}"),
            EngineError::DivideByZero => write!(f, "division by zero"),
            EngineError::IndexOutOfBounds(i) => write!(f, "array index out of bounds: {i}"),
            EngineError::PcOutOfRange(pc) => write!(f, "pc out of range: {pc}"),
            EngineError::StackUnderflow => write!(f, "value stack underflow"),
            EngineError::DeoptFailed(m) => write!(f, "deoptimization failed: {m}"),
            EngineError::OsrBadEntry(pc) => write!(f, "OSR entry not a loop header: {pc}"),
            EngineError::OutOfMemory => write!(f, "out of memory"),
            EngineError::GcNeeded => write!(f, "collection required before retry"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<crate::cold::bytecode::BytecodeError> for EngineError {
    fn from(e: crate::cold::bytecode::BytecodeError) -> Self {
        EngineError::Bytecode(e)
    }
}

impl From<crate::platform::TargetError> for EngineError {
    fn from(e: crate::platform::TargetError) -> Self {
        EngineError::Target(e)
    }
}

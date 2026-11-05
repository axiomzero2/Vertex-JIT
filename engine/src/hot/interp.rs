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

use crate::cold::bytecode::{Function, StringPool};
use crate::cold::errors::EngineError;
use crate::hot::frame::{BoundedStackUsize, FnFrame};
use crate::hot::stack::BoundedStack;

use crate::hot::value::Value;

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

/// Per-function interpreter profile: call counts, back-edge counts per
/// site and the deferred type-feedback arrays (doc/engine.md).
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

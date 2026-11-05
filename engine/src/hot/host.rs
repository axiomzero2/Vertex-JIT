// CEP:FILE: engine/src/hot/host.rs
// CEP:WHAT: Host I/O hooks for the interpreter: function-pointer write and
//           read-line callbacks plus the stdio defaults.
// CEP:WHY: Spec 25.6: the engine never touches the host directly; the
//          entry program injects its own hooks, and function-pointer
//          tables avoid trait-object dispatch in the hot path.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: hooks return engine errors; defaults never fail.
// CEP:ASSUMES: one host per engine instance.
// CEP:COST: one indirect call per I/O operation.
// CEP:EVIDENCE: differential suite; engine.rs wiring.
// CEP:SECURITY: the embedder owns every byte written through the hook.


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

/// CEP:WHAT: Default stdout write hook.
/// CEP:WHY: The stdio host lets example programs
///              and tests print without an embedder.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics if stdout is closed (a broken test host is loud).
/// CEP:ASSUMES: UTF-8 output.
/// CEP:COST: one write call.
/// CEP:EVIDENCE: example programs; differential suite default path.
fn default_write(s: &str) {
    print!("{s}");
}

/// Default stdin line hook.
/// CEP:WHAT: Default read_line hook: one stdin line, trimmed.
/// CEP:WHY: The stdio host lets example programs and tests read input
///              without an embedder.
/// CEP:STATUS: complete
/// CEP:FAILURE: read errors yield an empty line (the hook cannot
///              fail; the embedder installs stricter hooks).
/// CEP:ASSUMES: UTF-8 stdin.
/// CEP:COST: one read_line call.
/// CEP:EVIDENCE: example programs; differential suite default path.
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

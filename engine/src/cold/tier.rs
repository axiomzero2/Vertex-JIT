// CEP:FILE: engine/src/cold/tier.rs
// CEP:WHAT: Tier definitions, transition thresholds and JIT budgets.
// CEP:WHY: Spec 6.3 and 8.1 fix tier semantics and budget classes;
//          thresholds are named configuration, not magic numbers.
// CEP:CLASS: CEP-1
// CEP:STATUS: complete
// CEP:FAILURE: none; pure data.
// CEP:ASSUMES: none
// CEP:COST: zero at runtime.
// CEP:EVIDENCE: unit tests below.
// CEP:SECURITY: none

/// Compilation tier of an executable function body (spec 2-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Tier {
    /// Interpreter baseline; counters feed PGO.
    Interp = 0,
    /// Copy-and-patch, no IR (spec 2).
    T0 = 1,
    /// Sea of Nodes with basic optimizations (spec 3).
    T1 = 2,
    /// GVN/PEA/LICM (spec 4).
    T2 = 3,
    /// PGO, inlining, unrolling, vectorization (spec 5).
    T3 = 4,
}

impl Tier {
    /// Human-readable tier name.
    ///
    /// CEP:WHAT: Tier name.
    /// CEP:WHY: Diagnostics name tiers unambiguously.
    /// CEP:STATUS: complete
    /// CEP:FAILURE: none
    /// CEP:ASSUMES: none
    /// CEP:COST: O(1).
    /// CEP:EVIDENCE: stats formatting.
    /// CEP:SECURITY: none
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Tier::Interp => "interp",
            Tier::T0 => "t0",
            Tier::T1 => "t1",
            Tier::T2 => "t2",
            Tier::T3 => "t3",
        }
    }
}

/// Tier-up thresholds in invocation counts (spec 6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierThresholds {
    /// Calls at which a function compiles to T1 (spec: 10).
    pub t1_at: u64,
    /// Calls at which a function recompiles to T2 (spec: 100).
    pub t2_at: u64,
    /// Calls at which a function recompiles to T3 (spec: 1000).
    pub t3_at: u64,
    /// Back-edge count triggering OSR (spec 6.1 loop header checks).
    pub osr_at: u64,
}

impl Default for TierThresholds {
    fn default() -> Self {
        TierThresholds { t1_at: 10, t2_at: 100, t3_at: 1000, osr_at: 20_000 }
    }
}

/// JIT budgets (standard 38.39): explicit and enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitBudgets {
    /// Maximum resident machine-code bytes.
    pub code_cache_bytes: u32,
    /// Maximum IR nodes per compilation unit.
    pub max_ir_nodes: u32,
    /// Maximum inlining depth (spec 5.2.1: 3-5).
    pub max_inline_depth: u32,
    /// Maximum unroll factor (spec 5.3.1: 4-8x).
    pub max_unroll_factor: u32,
}

impl Default for JitBudgets {
    fn default() -> Self {
        JitBudgets {
            code_cache_bytes: 4 * 1024 * 1024,
            max_ir_nodes: 100_000,
            max_inline_depth: 4,
            max_unroll_factor: 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CEP:WHAT: Tier thresholds are strictly ordered.
/// CEP:WHY: The promotion ladder (Interp<T0<T1<T2<T3 budgets) must be
///              monotonic or tiering oscillates.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics on mismatch (the assertion is the failure path)
/// CEP:ASSUMES: none
/// CEP:COST: offline
/// CEP:EVIDENCE: threshold values asserted inline.
    #[test]
    fn tier_thresholds_ordering() {
        let t = TierThresholds::default();
        assert!(t.t1_at < t.t2_at && t.t2_at < t.t3_at);
        assert_eq!(Tier::T1.name(), "t1");
        assert!(Tier::T0 < Tier::T1 && Tier::T1 < Tier::T2 && Tier::T2 < Tier::T3);
    }
}

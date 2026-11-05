// CEP:FILE: engine/tests/stack_discipline/main.rs
// CEP:WHAT: Regression-matrix test target for the T0 stack-discipline and
//           IR-lift environment defect classes: module wiring plus the
//           cross-matrix quota invariant (50 cases each, standard 61.3).
// CEP:WHY: One test binary keeps the matrices a single CI signal; the
//          per-matrix files carry their own defect-class documentation.
// CEP:CLASS: CEP-2 (test tooling)
// CEP:STATUS: complete
// CEP:FAILURE: any assertion failure names the matrix and the quota.
// CEP:ASSUMES: each matrix file returns exactly its 50 cases.
// CEP:COST: offline.
// CEP:EVIDENCE: the quota test below runs on every CI change.
// CEP:SECURITY: test-authored bytecode only; no host input.


//! Regression matrices for the T0 stack-discipline defect classes
//! (standard 61.3: fifty tests per confirmed bug).

mod harness;
mod matrix_a;
mod matrix_b;
mod matrix_c;

/// CEP:WHAT: Cross-matrix quota invariant (standard 61.3).
/// CEP:WHY: A matrix that silently shrinks would weaken the regression guarantee
/// without any test failing; the quota check makes that loud.
/// CEP:STATUS: complete
/// CEP:FAILURE: panics naming the matrix and the quota.
/// CEP:ASSUMES: each matrix builder returns exactly its cases.
/// CEP:COST: O(150) case constructions.
/// CEP:EVIDENCE: runs on every CI change.
/// The matrices are exactly fifty cases each (standard 61.3 quota).
#[test]
fn matrices_are_fifty_cases_each() {
    assert_eq!(matrix_a::matrix_a().len(), 50, "matrix A quota");
    assert_eq!(matrix_b::matrix_b().len(), 50, "matrix B quota");
    assert_eq!(matrix_c::matrix_c().len(), 50, "matrix C quota");
}

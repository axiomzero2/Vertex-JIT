// CEP:FILE: tools/cep_lint/tests/cases/header_ok.cpp
// CEP:WHAT: Self-test case: a fully compliant file.
// CEP:WHY: The negative control for the header and block rules.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: assertion failures in the test harness only.
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.
#include "nothing.hpp"

namespace demo {

/// Multiplier scale.
constexpr long kScale = 3;

// CEP:WHAT: Compute a scaled value.
// CEP:WHY: Exercises the function-block rule on a clean function.
// CEP:STATUS: complete
// CEP:FAILURE: returns the input unchanged on overflow-free paths.
// CEP:ASSUMES: x fits long.
// CEP:COST: O(1).
// CEP:EVIDENCE: header_ok scenario.
auto scaled(long x) -> long {
    const long base = x + 1;
    return base * kScale;
}

}  // namespace demo

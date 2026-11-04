// CEP:FILE: tools/cep_lint/tests/cases/hot_banned.cpp
// CEP:WHAT: Self-test case: banned tokens in class-0 code.
// CEP:WHY: Exercises CEP-LINT-HOT-BANNED at severity 0.
// CEP:CLASS: CEP-0
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.

namespace demo {

// CEP:WHAT: Hot function with banned tokens.
// CEP:WHY: Class zero forbids allocation and exceptions.
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: this file.
auto hot(int x) -> int {
    int* p = new int(x);
    int v = *p;
    delete p;
    return v;
}

}  // namespace demo

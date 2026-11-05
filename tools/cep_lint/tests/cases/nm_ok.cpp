// CEP:FILE: tools/cep_lint/tests/cases/nm_ok.cpp
// CEP:WHAT: PascalCase type, snake_case functions.
// CEP:WHY: Self-test fixture.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: panics on divergence (test fixture).
// CEP:ASSUMES: none
// CEP:COST: O(1).
// CEP:EVIDENCE: the self-test manifest.
struct Shape {
    int sides;
};
/// CEP:WHAT: fixture function
/// CEP:WHY: fixture
/// CEP:STATUS: complete
/// CEP:FAILURE: none
/// CEP:ASSUMES: none
/// CEP:COST: O(1).
/// CEP:EVIDENCE: manifest
int area_of(Shape s) {
    return s.sides;
}

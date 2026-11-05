// CEP:FILE: tools/cep_lint/tests/cases/enum_class_clean.cpp
// CEP:WHAT: Self-test case: PascalCase enum class names are compliant.
// CEP:WHY: Pins the repair of the NAMING-TYPE backtracking defect that
//           flagged every `enum class` as `enum` + name `class`.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: none
// CEP:ASSUMES: none
// CEP:COST: offline.
// CEP:EVIDENCE: this file.

namespace demo {

enum class Kind { Object, Array, String };

enum class ReaderState { Code, Comment, Str };

struct WellFormed {
    Kind kind;
};

}  // namespace demo

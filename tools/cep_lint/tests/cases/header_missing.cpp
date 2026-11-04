// A file whose first block is a plain comment, not a CEP header.

namespace demo {

constexpr long kOne = 1;

// CEP:WHAT: Tiny function below the statement threshold.
// CEP:STATUS: complete
auto tiny() -> long {
    return kOne;
}

}  // namespace demo

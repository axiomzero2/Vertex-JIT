// CEP:FILE: tools/cep_lint/src/reader.hpp
// CEP:WHAT: Shared C++ reader: comment/string-aware lexing, function
//           extraction and comment-block queries.
// CEP:WHY: The check primitives all consume the same stripped view;
//           line numbers must stay exact (findings carry them).
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: Malformed sources lex as plain text; no failure path.
// CEP:ASSUMES: UTF-8; raw strings R"del(...)del".
// CEP:COST: O(bytes) single pass.
// CEP:EVIDENCE: self-test scenarios with nested strings and comments.
// CEP:SECURITY: none
#pragma once

#include <string>
#include <vector>

namespace cep {

/// C++ source views of one file: raw comments, stripped code, strings.
struct SourceView {
    /// Path as given.
    std::string path;
    /// Raw lines without terminators.
    std::vector<std::string> lines;
    /// Per line: comment segment texts.
    std::vector<std::vector<std::string>> comments;
    /// Per line: code with comments and string contents blanked.
    std::vector<std::string> code;
    /// Per line: string literal contents.
    std::vector<std::vector<std::string>> strings;
    /// Declared CEP:CLASS value (first token), empty when absent.
    std::string declared_class;
};

/// Lex a C++ source into views; line numbers stay exact.
auto lex_source(const std::string& path, const std::string& text, SourceView& out) -> void;

/// True when the line holds no code (comment or blank).
auto is_comment_or_blank(const SourceView& v, std::size_t idx) -> bool;

/// Function definition extracted by the reader.
struct FuncDef {
    /// Declared name (empty when not recoverable).
    std::string name;
    /// First line of the signature (0-based).
    long header_line = 0;
    /// Line of the opening brace (0-based).
    long body_line = 0;
    /// Line of the closing brace (0-based).
    long end_line = 0;
    /// Statements at paren depth zero inside the body.
    long statements = 0;
    /// True when the body holds no statement.
    bool empty = false;
    /// True for destructors and operators (naming exempt).
    bool exempt_name = false;
};

/// Extract function definitions from the stripped code view.
auto extract_functions(const SourceView& v) -> std::vector<FuncDef>;

/// Concatenate the full-line comment block ending just above `line`.
/// Yields the block's first line (0-based) and the joined text.
auto block_above(const SourceView& v, long line, long& block_start, std::string& text) -> bool;

/// True when the comment text contains a `FIELD:` marker.
auto has_field(const std::string& block, const std::string& field) -> bool;

/// Trimmed value of a `FIELD:` marker; empty when absent.
auto field_value(const std::string& block, const std::string& field) -> std::string;

/// True when the comment line is a CEP field line (sanctioned form).
auto is_cep_field_line(const std::string& cmt) -> bool;

/// Parse a numeric literal (suffixes stripped) to a double.
auto literal_value(const std::string& lit, double& out) -> bool;

}  // namespace cep

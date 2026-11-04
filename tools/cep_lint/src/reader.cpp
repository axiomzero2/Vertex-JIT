// CEP:FILE: tools/cep_lint/src/reader.cpp
// CEP:WHAT: Comment/string-aware C++ lexing with exact line numbers,
//           function extraction and comment-block queries.
// CEP:WHY: Every check primitive consumes the same stripped view; findings
//          carry line numbers, so lexing must not shift lines.
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: Malformed sources lex as plain text; no failure path.
// CEP:ASSUMES: UTF-8; raw strings of the R"del(...)del" form.
// CEP:COST: O(bytes) single pass.
// CEP:EVIDENCE: self-test scenarios with nested strings and comments.
// CEP:SECURITY: none
#include "reader.hpp"

#include <cctype>
#include <regex>
#include <cstdio>
#include <cstdlib>
#include <string>

namespace cep {

/// Length of two-character operators (// and /*).
constexpr std::size_t kTwoChars = 2;
/// The class field marker.
constexpr const char* kClassField = "CEP:CLASS:";


// CEP:WHAT: Parse a numeric literal (suffixes stripped) to a double.
// CEP:WHY: Numeric comparisons in the config are value-based, not textual.
// CEP:STATUS: complete
// CEP:FAILURE: false when the text holds no parsable number.
// CEP:ASSUMES: the literal fits double precision.
// CEP:COST: O(length).
// CEP:EVIDENCE: self-test numeric scenarios.
auto literal_value(const std::string& lit, double& out) -> bool {
    std::string t;
    for (const char c : lit) {
        const bool suffix = c == '\'' || c == 'u' || c == 'U' || c == 'l' || c == 'L' || c == 'f'
                            || c == 'F';
        if (!suffix) {
            t.push_back(c);
        }
    }
    try {
        std::size_t used = 0;
        const double val = std::stod(t, &used);
        if (used == 0) {
            return false;
        }
        out = val;
        return true;
    } catch (const std::exception&) {
        return false;
    }
}

// CEP:WHAT: True when the comment text contains a FIELD marker.
// CEP:WHY: Header and block completeness checks share the marker test.
// CEP:STATUS: complete
// CEP:FAILURE: none.
// CEP:ASSUMES: markers are uppercase with a colon.
// CEP:COST: O(text).
// CEP:EVIDENCE: self-test header scenarios.
auto has_field(const std::string& block, const std::string& field) -> bool {
    return block.find(field + ":") != std::string::npos;
}

// CEP:WHAT: Trimmed value of a FIELD marker; empty when absent.
// CEP:WHY: Status and class values are compared after trimming.
// CEP:STATUS: complete
// CEP:FAILURE: empty result when the marker is absent.
// CEP:ASSUMES: the value ends at the line end.
// CEP:COST: O(marker line).
// CEP:EVIDENCE: self-test header scenarios.
auto field_value(const std::string& block, const std::string& field) -> std::string {
    const std::string key = field + ":";
    const std::size_t at = block.find(key);
    if (at == std::string::npos) {
        return std::string();
    }
    std::string val = block.substr(at + key.size());
    const std::size_t eol = val.find('\n');
    if (eol != std::string::npos) {
        val = val.substr(0, eol);
    }
    const std::size_t b = val.find_first_not_of(" \t");
    if (b == std::string::npos) {
        return std::string();
    }
    const std::size_t e = val.find_last_not_of(" \t\r");
    return val.substr(b, e - b + 1);
}

// CEP:WHAT: True when the line holds no code (comment or blank).
// CEP:WHY: Full-line comment blocks and blank runs share this predicate.
// CEP:STATUS: complete
// CEP:FAILURE: none.
// CEP:ASSUMES: idx within or one past the view.
// CEP:COST: O(line).
// CEP:EVIDENCE: self-test scenarios.
auto is_comment_or_blank(const SourceView& v, std::size_t idx) -> bool {
    if (idx >= v.code.size()) {
        return true;
    }
    return v.code[idx].find_first_not_of(" \t\r") == std::string::npos;
}

// CEP:WHAT: Lex a C++ source into views; line numbers stay exact.
// CEP:WHY: All primitives read the same comment/string-separated view.
// CEP:STATUS: complete
// CEP:FAILURE: malformed sources lex as plain text; no failure path.
// CEP:ASSUMES: UTF-8 input; raw strings R"del(...)del".
// CEP:COST: O(bytes) single pass.
// CEP:EVIDENCE: self-test scenarios with nested strings and comments.
auto lex_source(const std::string& path, const std::string& text, SourceView& out) -> void {
    out = SourceView{};
    out.path = path;
    std::string cur_code;
    std::vector<std::string> cur_comments;
    std::vector<std::string> cur_strings;

    auto flush = [&]() {
        out.code.push_back(cur_code);
        out.comments.push_back(cur_comments);
        out.strings.push_back(cur_strings);
        cur_code.clear();
        cur_comments.clear();
        cur_strings.clear();
    };

    /// Lexer states.
    enum class St { Code, LineComment, BlockComment, Str, Char, RawStr };
    St st = St::Code;
    std::string raw_delim;
    std::string block_acc;
    std::string lit_acc;
    std::size_t i = 0;
    while (i < text.size()) {
        const char c = text[i];
        const char next = (i + 1 < text.size()) ? text[i + 1] : '\0';
        switch (st) {
            case St::Code:
                if (c == '/' && next == '/') {
                    st = St::LineComment;
                    block_acc.clear();
                    i += kTwoChars;
                    continue;
                }
                if (c == '/' && next == '*') {
                    st = St::BlockComment;
                    block_acc.clear();
                    i += kTwoChars;
                    continue;
                }
                if (c == 'R' && next == '"') {
                    std::size_t k = i + kTwoChars;
                    std::string delim;
                    while (k < text.size() && text[k] != '(' && text[k] != '"') {
                        delim.push_back(text[k]);
                        ++k;
                    }
                    if (k < text.size() && text[k] == '(') {
                        st = St::RawStr;
                        raw_delim = delim;
                        cur_code += "\"\"";
                        i = k + 1;
                        continue;
                    }
                    cur_code.push_back(c);
                    ++i;
                    continue;
                }
                if (c == '"') {
                    st = St::Str;
                    lit_acc.clear();
                    cur_code.push_back('"');
                    ++i;
                    continue;
                }
                if (c == '\'') {
                    st = St::Char;
                    cur_code.push_back('\'');
                    ++i;
                    continue;
                }
                if (c == '\n') {
                    flush();
                    ++i;
                    continue;
                }
                cur_code.push_back(c);
                ++i;
                continue;
            case St::LineComment:
                if (c == '\n') {
                    cur_comments.push_back(block_acc);
                    flush();
                    st = St::Code;
                    ++i;
                    continue;
                }
                block_acc.push_back(c);
                ++i;
                continue;
            case St::BlockComment:
                if (c == '*' && next == '/') {
                    cur_comments.push_back(block_acc);
                    block_acc.clear();
                    st = St::Code;
                    i += kTwoChars;
                    continue;
                }
                if (c == '\n') {
                    cur_comments.push_back(block_acc);
                    block_acc.clear();
                    flush();
                    ++i;
                    continue;
                }
                block_acc.push_back(c);
                ++i;
                continue;
            case St::Str:
                if (c == '\\' && i + 1 < text.size()) {
                    lit_acc.push_back(c);
                    lit_acc.push_back(text[i + 1]);
                    i += kTwoChars;
                    continue;
                }
                if (c == '"' || c == '\n') {
                    st = St::Code;
                    cur_strings.push_back(lit_acc);
                    cur_code.push_back('"');
                    ++i;
                    continue;
                }
                lit_acc.push_back(c);
                ++i;
                continue;
            case St::Char:
                if (c == '\\' && i + 1 < text.size()) {
                    i += kTwoChars;
                    continue;
                }
                if (c == '\'' || c == '\n') {
                    st = St::Code;
                    cur_code.push_back('\'');
                    ++i;
                    continue;
                }
                ++i;
                continue;
            case St::RawStr:
                if (c == ')' && text.compare(i + 1, raw_delim.size(), raw_delim) == 0) {
                    st = St::Code;
                    i += raw_delim.size() + kTwoChars;
                    continue;
                }
                if (c == '\n') {
                    flush();
                }
                ++i;
                continue;
        }
    }
    if (!cur_code.empty() || !cur_comments.empty()) {
        if (st == St::LineComment) {
            cur_comments.push_back(block_acc);
        }
        flush();
    }
    // Declared class: first CEP:CLASS: occurrence in any comment.
    for (const auto& line_comments : out.comments) {
        for (const std::string& cmt : line_comments) {
            const std::size_t at = cmt.find(kClassField);
            if (at != std::string::npos) {
                std::string val = cmt.substr(at + std::string(kClassField).size());
                const std::size_t b = val.find_first_not_of(" \t");
                if (b == std::string::npos) {
                    continue;
                }
                val = val.substr(b);
                const std::size_t e = val.find_first_of(" \t(,");
                out.declared_class = (e == std::string::npos) ? val : val.substr(0, e);
                return;
            }
        }
    }
}

namespace {

// CEP:WHAT: True when the trimmed header text is a function signature.
// CEP:WHY: The extractor must reject control-flow and class headers.
// CEP:STATUS: complete
// CEP:FAILURE: false for non-signature headers.
// CEP:ASSUMES: ECMAScript regex grammar.
// CEP:COST: O(header).
// CEP:EVIDENCE: self-test function scenarios.
auto signature_match(const std::string& trimmed, std::smatch& m) -> bool {
    static const std::regex control_re(
        "(^|[^A-Za-z_])(if|for|while|switch|catch|else|do|return|sizeof|alignof|decltype|"
        "static_assert|namespace|struct|class|enum|union)\\s*\\(");
    static const std::regex sig_re(
        "([A-Za-z_][A-Za-z0-9_:]*)\\s*\\([^()]*\\)\\s*(const\\s*)?(noexcept\\s*)?(override"
        "\\s*)?(final\\s*)?(->\\s*[A-Za-z0-9_:<>\\*&\\s]+\\s*)?$");
    if (trimmed.empty() || std::regex_search(trimmed, control_re)) {
        return false;
    }
    return std::regex_search(trimmed, m, sig_re);
}

}  // namespace

// CEP:WHAT: Extract function definitions from the stripped code view.
// CEP:WHY: FUNC-BLOCK, STUB-BODY, CONSTANT-RETURN and NAMING-FUNC all
//          consume the same definition list.
// CEP:STATUS: complete
// CEP:FAILURE: lambdas, control headers and class bodies are not returned.
// CEP:ASSUMES: brace-balanced input; approximate statement counting per
//              standard 50.3 limits.
// CEP:COST: O(bytes).
// CEP:EVIDENCE: self-test function scenarios.
auto extract_functions(const SourceView& v) -> std::vector<FuncDef> {
    std::vector<FuncDef> out;
    std::string flat;
    std::vector<long> line_of;
    for (std::size_t li = 0; li < v.code.size(); ++li) {
        for (const char c : v.code[li]) {
            flat.push_back(c);
            line_of.push_back(static_cast<long>(li));
        }
        flat.push_back(' ');
        line_of.push_back(static_cast<long>(li));
    }
    std::size_t header_begin = 0;
    std::size_t depth = 0;
    std::size_t i = 0;
    while (i < flat.size()) {
        const char c = flat[i];
        if (c == '{') {
            std::size_t first = header_begin;
            while (first < i && (flat[first] == ' ' || flat[first] == '\t' || flat[first] == '\n')) {
                ++first;
            }
            const std::string trimmed = flat.substr(first, i - first);
            std::smatch m;
            std::string probe;
            for (const char ch : trimmed) {
                if (ch != ' ' && ch != '\t' && ch != '\n') {
                    probe.push_back(ch);
                }
            }
            const bool lambda_end =
                probe.size() >= kTwoChars && probe.compare(probe.size() - kTwoChars, kTwoChars, "]{") == 0;
            if (!lambda_end && signature_match(trimmed, m)) {
                const std::string fname = m[1].str();
                std::size_t d = 1;
                std::size_t j = i + 1;
                long statements = 0;
                long parens = 0;
                while (j < flat.size() && d > 0) {
                    const char b = flat[j];
                    if (b == '(') {
                        ++parens;
                    } else if (b == ')') {
                        --parens;
                    } else if (b == ';' && parens == 0) {
                        ++statements;
                    } else if (b == '{') {
                        ++d;
                    } else if (b == '}') {
                        --d;
                    }
                    ++j;
                }
                FuncDef fd;
                fd.name = fname;
                fd.header_line = line_of[first];
                fd.body_line = line_of[i];
                fd.end_line = (j - 1 < line_of.size()) ? line_of[j - 1] : line_of.back();
                fd.statements = statements;
                fd.empty = statements == 0;
                fd.exempt_name =
                    !fname.empty() && (fname[0] == '~' || fname.rfind("operator", 0) == 0);
                out.push_back(std::move(fd));
            }
            ++depth;
            ++i;
            header_begin = i;
            continue;
        }
        if (c == '}') {
            if (depth > 0) {
                --depth;
            }
            ++i;
            header_begin = i;
            continue;
        }
        // Every semicolon starts a new header candidate; the depth guard
        // would never fire inside a namespace (the namespace brace never
        // closes mid-file) and headers would span prior declarations.
        if (c == ';') {
            ++i;
            header_begin = i;
            continue;
        }
        ++i;
    }
    return out;
}

// CEP:WHAT: Concatenate the full-line comment block ending just above a line.
// CEP:WHY: FUNC-BLOCK and STUB-BODY read the block preceding a definition.
// CEP:STATUS: complete
// CEP:FAILURE: false when no comment block exists above.
// CEP:ASSUMES: blank lines between the block and the definition are skipped.
// CEP:COST: O(block).
// CEP:EVIDENCE: self-test function scenarios.
auto block_above(const SourceView& v, long line, long& block_start, std::string& text) -> bool {
    long idx = line - 1;
    while (idx >= 0 && is_comment_or_blank(v, static_cast<std::size_t>(idx))
           && v.comments[static_cast<std::size_t>(idx)].empty()) {
        --idx;
    }
    if (idx < 0 || v.comments[static_cast<std::size_t>(idx)].empty()) {
        return false;
    }
    const long end = idx;
    std::vector<std::string> parts;
    while (idx >= 0 && !v.comments[static_cast<std::size_t>(idx)].empty()
           && is_comment_or_blank(v, static_cast<std::size_t>(idx))) {
        for (const std::string& c : v.comments[static_cast<std::size_t>(idx)]) {
            parts.push_back(c);
        }
        --idx;
    }
    block_start = idx + 1;
    for (std::size_t k = 0; k < parts.size(); ++k) {
        if (k > 0) {
            text.push_back('\n');
        }
        text += parts[k];
    }
    return end >= block_start;
}

}  // namespace cep

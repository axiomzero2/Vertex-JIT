// CEP:FILE: tools/cep_lint/src/config.cpp
// CEP:WHAT: Configuration loading: file reads, deep merge, schema
//           validation and deterministic directory listing.
// CEP:WHY: Standard 50: the engine is data-driven; loading failures are
//          coded bootstrap errors (CEP-LINT-BOOT-1..5).
// CEP:CLASS: CEP-2
// CEP:STATUS: complete
// CEP:FAILURE: Missing keys (BOOT-4), wrong types (BOOT-5), unreadable
//              files (BOOT-1); every failure carries the origin path.
// CEP:ASSUMES: JSON per src/json.hpp.
// CEP:COST: O(config size).
// CEP:EVIDENCE: self-test scenario configs exercise merge and defaults.
// CEP:SECURITY: reads only the paths handed to it.
#include "lint.hpp"

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <regex>
#include <sstream>

namespace cep {

namespace fs = std::filesystem;

// CEP:WHAT: Read a whole file into a string; false when unreadable.
// CEP:WHY: Every file access funnels through one auditable reader.
// CEP:STATUS: complete
// CEP:FAILURE: false when the file cannot be opened (BOOT-1 upstream).
// CEP:ASSUMES: the path is readable as a binary stream.
// CEP:COST: O(bytes).
// CEP:EVIDENCE: self-test bootstrap scenarios.
auto read_file(const std::string& path, std::string& out) -> bool {
    std::ifstream f(path, std::ios::binary);
    if (!f.is_open()) {
        return false;
    }
    std::ostringstream ss;
    ss << f.rdbuf();
    out = ss.str();
    return true;
}

// CEP:WHAT: Deep-merge overlay into base (objects merge, arrays replace).
// CEP:WHY: Self-test scenarios overlay the base configuration without
//          duplicating it.
// CEP:STATUS: complete
// CEP:FAILURE: none; non-object overlays replace wholesale.
// CEP:ASSUMES: acyclic inputs (JSON cannot cycle).
// CEP:COST: O(merged size).
// CEP:EVIDENCE: self-test scenario configs.
auto json_merge(const Json& base, const Json& overlay) -> Json {
    if (base.is_object() && overlay.is_object()) {
        Json out = base;
        for (const auto& kv : overlay.members) {
            const Json* b = out.find(kv.first);
            if (b != nullptr) {
                Json merged = json_merge(*b, kv.second);
                for (auto& m : out.members) {
                    if (m.first == kv.first) {
                        m.second = std::move(merged);
                    }
                }
            } else {
                out.members.emplace_back(kv.first, kv.second);
            }
        }
        return out;
    }
    return overlay;
}

// CEP:WHAT: Build a Config from a parsed JSON object.
// CEP:WHY: Schema validation in one place keeps the engine mechanism-only.
// CEP:STATUS: complete
// CEP:FAILURE: BOOT-4 missing keys, BOOT-5 wrong types, BOOT-7 severity
//              out of range, BOOT-6 missing check primitive.
// CEP:ASSUMES: the document follows the .cep/cep_lint.json schema.
// CEP:COST: O(rules).
// CEP:EVIDENCE: self-test bootstrap scenarios.
auto config_from_json(const Json& j, const std::string& origin, Config& out, std::string& err)
    -> bool {
    const Json* std_obj = j.find("standard");
    if (std_obj == nullptr) {
        err = "CEP-LINT-BOOT-4: missing key 'standard' in " + origin;
        return false;
    }
    out.standard_version = std_obj->str("version");
    out.doc_path = std_obj->str("document");
    if (out.standard_version.empty() || out.doc_path.empty()) {
        err = "CEP-LINT-BOOT-4: standard.version and standard.document required in " + origin;
        return false;
    }
    out.fail_on = j.int_array("fail_on");
    if (out.fail_on.empty()) {
        out.fail_on = {kDefaultFailSeverity0, kDefaultFailSeverity1};
    }
    out.file_globs = j.str_array("file_globs");
    const Json* rules = j.find("rules");
    if (rules == nullptr || !rules->is_object()) {
        err = "CEP-LINT-BOOT-4: missing key 'rules' in " + origin;
        return false;
    }
    for (const auto& kv : rules->members) {
        if (!kv.second.is_object()) {
            err = "CEP-LINT-BOOT-5: rule " + kv.first + " must be an object in " + origin;
            return false;
        }
        Rule r;
        r.key = kv.first;
        r.severity = kv.second.integer("severity", kInvalidSeverity);
        if (r.severity < kMinSeverity || r.severity > kMaxSeverity) {
            err = "CEP-LINT-BOOT-7: rule " + kv.first + " severity out of range in " + origin;
            return false;
        }
        r.check = kv.second.str("check");
        if (r.check.empty()) {
            err = "CEP-LINT-BOOT-6: rule " + kv.first + " missing 'check' in " + origin;
            return false;
        }
        r.mode = kv.second.str("mode");
        r.params = kv.second;
        out.rules.push_back(std::move(r));
    }
    return true;
}

// CEP:WHAT: Load and validate the configuration (BOOT-2..8 on failure).
// CEP:WHY: The entry point for -c and the self-test base configuration.
// CEP:STATUS: complete
// CEP:FAILURE: BOOT-1 unreadable file, BOOT-2 parse failure; schema
//              failures propagate from config_from_json.
// CEP:ASSUMES: the path points at the lint configuration JSON.
// CEP:COST: O(config).
// CEP:EVIDENCE: self-test bootstrap scenarios.
auto load_config(const std::string& path, Config& out, std::string& err) -> bool {
    std::string text;
    if (!read_file(path, text)) {
        err = "CEP-LINT-BOOT-1: cannot read configuration " + path;
        return false;
    }
    std::optional<Json> j = json_parse(text, path);
    if (!j.has_value()) {
        err = "CEP-LINT-BOOT-2: " + json_last_error();
        return false;
    }
    return config_from_json(*j, path, out, err);
}

// CEP:WHAT: List files under a directory (sorted, recursive) or the file.
// CEP:WHY: Directory traversal must be sorted for deterministic output.
// CEP:STATUS: complete
// CEP:FAILURE: empty result for unreadable paths (BOOT-1 upstream).
// CEP:ASSUMES: filesystem traversal is ordered after the final sort.
// CEP:COST: O(entries).
// CEP:EVIDENCE: self-test and CI usage.
auto collect_files(const std::string& path, const std::vector<std::string>& exts)
    -> std::vector<std::string> {
    std::vector<std::string> out;
    std::error_code ec;
    if (fs::is_regular_file(path, ec)) {
        out.push_back(path);
        return out;
    }
    if (!fs::is_directory(path, ec)) {
        return out;
    }
    std::vector<std::string> all;
    for (const fs::directory_entry& e : fs::recursive_directory_iterator(path)) {
        if (!e.is_regular_file()) {
            continue;
        }
        std::string p = e.path().string();
        const std::string name = e.path().filename().string();
        if (name.rfind('.', 0) == 0) {
            continue;
        }
        bool ext_ok = exts.empty();
        for (const std::string& ext : exts) {
            if (p.size() >= ext.size()
                && p.compare(p.size() - ext.size(), ext.size(), ext) == 0) {
                ext_ok = true;
            }
        }
        if (ext_ok) {
            all.push_back(p);
        }
    }
    std::sort(all.begin(), all.end());
    out = all;
    return out;
}

namespace {

// CEP:WHAT: Translate one glob to an ECMAScript regex.
// CEP:WHY: `**` spans any number of directories, `*` stays in one segment.
// CEP:STATUS: complete
// CEP:FAILURE: invalid patterns surface through compile_re upstream.
// CEP:ASSUMES: '/' separators on every platform.
// CEP:COST: O(pattern).
// CEP:EVIDENCE: glob scenarios in the self-test manifest.
auto glob_to_regex(const std::string& glob, std::string& out) -> void {
    out.clear();
    for (std::size_t i = 0; i < glob.size(); ++i) {
        const char c = glob[i];
        if (c == '*') {
            const bool double_star = (i + 1 < glob.size()) && glob[i + 1] == '*';
            if (double_star) {
                ++i;
                // Consume one following '/' so `a/**/b` also matches `a/b`.
                if (i + 1 < glob.size() && glob[i + 1] == '/') {
                    ++i;
                    out += "(?:.*/)?";
                } else {
                    out += ".*";
                }
            } else {
                out += "[^/]*";
            }
            continue;
        }
        if (c == '?') {
            out += "[^/]";
            continue;
        }
        if (std::string(".^$|()[]{}+\\").find(c) != std::string::npos) {
            out.push_back('\\');
        }
        out.push_back(c);
    }
}

// CEP:WHAT: Recursively list candidate files under the working directory.
// CEP:WHY: Glob expansion needs relative paths; traversal must skip VCS
//          and build trees for determinism and cost.
// CEP:STATUS: complete
// CEP:FAILURE: unreadable directories are skipped silently.
// CEP:ASSUMES: the working directory is the repository root.
// CEP:COST: O(entries under root).
// CEP:EVIDENCE: the no-target scan gate in CI.
auto walk_relative(const fs::path& dir, const std::string& prefix,
                   std::vector<std::string>& out) -> void {
    std::error_code ec;
    for (const fs::directory_entry& e : fs::directory_iterator(dir, ec)) {
        const std::string name = e.path().filename().string();
        if (name.rfind('.', 0) == 0 || name == "target" || name == "node_modules") {
            continue;
        }
        const std::string rel = prefix.empty() ? name : prefix + "/" + name;
        std::error_code ec2;
        const bool dir_p = e.is_directory(ec2);
        if (dir_p) {
            walk_relative(e.path(), rel, out);
        } else if (e.is_regular_file(ec2)) {
            out.push_back(rel);
        }
    }
}

}  // namespace

// CEP:WHAT: Expand configuration file_globs from the working directory.
// CEP:WHY: The no-target scan mode lints exactly the declared first-party
//          set (0.4 applicability note), deterministically sorted.
// CEP:STATUS: complete
// CEP:FAILURE: globs matching nothing yield an empty result; the caller
//              fails closed with BOOT-1.
// CEP:ASSUMES: the working directory is the repository root.
// CEP:COST: O(entries x globs).
// CEP:EVIDENCE: the no-target scan gate in CI.
auto collect_globs(const std::vector<std::string>& globs,
                   const std::vector<std::string>& exts) -> std::vector<std::string> {
    std::vector<std::string> all;
    walk_relative(".", "", all);
    std::sort(all.begin(), all.end());
    std::vector<std::string> out;
    for (const std::string& g : globs) {
        if (g.find('*') == std::string::npos && g.find('?') == std::string::npos) {
            // Literal path: a file or a directory expanded by extension.
            for (const std::string& f : collect_files(g, exts)) {
                out.push_back(f);
            }
            continue;
        }
        std::string pat;
        glob_to_regex(g, pat);
        std::regex re;
        try {
            re.assign(pat);
        } catch (const std::regex_error&) {
            continue;
        }
        for (const std::string& rel : all) {
            if (std::regex_match(rel, re)) {
                bool ext_ok = exts.empty();
                for (const std::string& ext : exts) {
                    if (rel.size() >= ext.size()
                        && rel.compare(rel.size() - ext.size(), ext.size(), ext) == 0) {
                        ext_ok = true;
                    }
                }
                if (ext_ok) {
                    out.push_back(rel);
                }
            }
        }
    }
    std::sort(out.begin(), out.end());
    out.erase(std::unique(out.begin(), out.end()), out.end());
    return out;
}

}  // namespace cep

#!/bin/sh
# tools/cep_lint/build.sh
# CEP:WHAT: Deterministic single-translation-unit build of the linter.
# CEP:WHY: Standard 50.6: the tool compiles with the section 6.3 warning
#          set and -Werror; MODE=asan builds the sanitizer self-test.
# CEP:CLASS: CEP-2
# CEP:STATUS: complete
# CEP:FAILURE: Compilation failures fail the script loudly (set -e).
# CEP:ASSUMES: g++ or clang++ with -std=c++26 support.
# CEP:COST: one compile of four translation units.
# CEP:EVIDENCE: CI runs this script before the self-lint gate.
# CEP:SECURITY: no network, no inputs beyond the source tree.
set -e
cd "$(dirname "$0")"

CXX="${CXX:-g++}"
WARN="-Wall -Wextra -Wpedantic -Wshadow -Wconversion -Wsign-conversion -Werror"
STD="-std=c++26 -O2"

if [ "${MODE:-}" = "asan" ]; then
    STD="-std=c++26 -O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer"
    # libstdc++ <regex> internals trip maybe-uninitialized under the
    # sanitizer optimizer (GCC 14, std::function storage); the warning is
    # scoped to this mode, the normal build keeps the full warning set.
    WARN="$WARN -Wno-maybe-uninitialized"
fi

mkdir -p build
$CXX $STD $WARN -o build/cep_lint \
    src/json.cpp \
    src/config.cpp \
    src/reader.cpp \
    src/checks.cpp \
    src/main.cpp
echo "built build/cep_lint"

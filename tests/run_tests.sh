#!/usr/bin/env bash
# Black-box test suite for the minishell binary.
# Builds the project, then pipes shell scripts into it and checks the
# resulting output and/or filesystem state.

set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$PROJECT_ROOT/target/debug/MiniShell_Rust"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

RED=$'\033[31m'
GREEN=$'\033[32m'
NC=$'\033[0m'

PASS=0
FAIL=0

echo "Building minishell..."
if ! (cd "$PROJECT_ROOT" && cargo build -q); then
    echo "${RED}Build failed${NC}"
    exit 1
fi

# run_case NAME INPUT EXPECTED_PATTERN [EXTRA_CHECK]
#
#   INPUT             text piped into the shell's stdin
#   EXPECTED_PATTERN  extended regex checked against stdout+stderr;
#                     pass "" to skip this check entirely
#   EXTRA_CHECK       optional shell command, eval'd in WORKDIR, that must
#                     exit 0. Defaults to "true". Can read the captured
#                     output via $ACTUAL_OUTPUT (e.g. to assert something
#                     was NOT printed) and the shell's exit code via
#                     $ACTUAL_EXIT_CODE, and inspect files left in WORKDIR.
run_case() {
    local name="$1" input="$2" expected="$3" extra_check="${4:-true}"
    local actual stdout_ok=0 extra_ok=0

    actual=$(cd "$WORKDIR" && timeout 5 "$BIN" <<< "$input" 2>&1)
    ACTUAL_EXIT_CODE=$?
    ACTUAL_OUTPUT="$actual"

    if [[ -z "$expected" ]]; then
        stdout_ok=1
    else
        grep -qE "$expected" <<< "$actual" && stdout_ok=1
    fi
    ( cd "$WORKDIR" && eval "$extra_check" ) >/dev/null 2>&1 && extra_ok=1

    if [[ $stdout_ok -eq 1 && $extra_ok -eq 1 ]]; then
        PASS=$((PASS + 1))
        echo "${GREEN}PASS${NC} - $name"
    else
        FAIL=$((FAIL + 1))
        echo "${RED}FAIL${NC} - $name"
        echo "  input:    $input"
        [[ $stdout_ok -eq 0 ]] && echo "  expected output to match: $expected"
        [[ $extra_ok -eq 0 ]] && echo "  extra check failed:       $extra_check"
        echo "  actual output:"
        sed 's/^/    /' <<< "$actual"
    fi
}

echo
echo "== Simple commands =="
run_case "simple command" \
    'echo hello' \
    'hello'

run_case "command with args" \
    'echo one two three' \
    'one two three'

run_case "unknown command reports an error" \
    'this_command_does_not_exist' \
    'Execution Error'

echo
echo "== Logical operators =="
run_case "&& runs the right side on success" \
    'true && echo yes' \
    'yes'

run_case "&& skips the right side on failure" \
    'false && echo should_not_print' \
    '' \
    '! grep -q should_not_print <<< "$ACTUAL_OUTPUT"'

run_case "|| runs the right side on failure" \
    'false || echo fallback' \
    'fallback'

run_case "|| skips the right side on success" \
    'true || echo should_not_print' \
    '' \
    '! grep -q should_not_print <<< "$ACTUAL_OUTPUT"'

echo
echo "== Pipelines =="
run_case "two-stage pipeline" \
    'echo hello | cat' \
    'hello'

run_case "three-stage pipeline" \
    'echo hello | cat | cat' \
    'hello'

touch "$WORKDIR/needle_file" "$WORKDIR/other_file"
run_case "pipeline filters ls output through grep" \
    'ls | grep needle_file' \
    'needle_file' \
    '! grep -q other_file <<< "$ACTUAL_OUTPUT"'

echo
echo "== Redirections =="
rm -f "$WORKDIR/outfile"
run_case "> creates and writes a file" \
    'echo hello > outfile' \
    '' \
    'grep -q hello outfile'

run_case ">> appends to a file" \
    'echo world >> outfile' \
    '' \
    'grep -q hello outfile && grep -q world outfile'

run_case "< feeds a file into stdin" \
    'cat < outfile' \
    'hello'

rm -f "$WORKDIR/filtered"
run_case "pipeline output redirected to a file" \
    'echo needle | cat > filtered' \
    '' \
    'grep -q needle filtered'

echo
echo "== if / elif / else =="
run_case "if takes the then branch" \
    'if true; then echo A; else echo B; fi' \
    '\bA\b'

run_case "if falls through to else" \
    'if false; then echo A; else echo B; fi' \
    '\bB\b'

run_case "elif branch is taken" \
    'if false; then echo A; elif true; then echo B; else echo C; fi' \
    '\bB\b'

run_case "all conditions false falls to else" \
    'if false; then echo A; elif false; then echo B; else echo C; fi' \
    '\bC\b'

echo
echo "== while / until =="
rm -f "$WORKDIR/statefile"
touch "$WORKDIR/statefile"
run_case "while loop runs until its condition fails" \
    'while ls statefile; do rm statefile; done' \
    'statefile' \
    '[ ! -f statefile ]'

rm -f "$WORKDIR/statefile2"
run_case "until loop runs until its condition succeeds" \
    'until ls statefile2; do touch statefile2; done' \
    'statefile2' \
    '[ -f statefile2 ]'

echo
echo "== for =="
run_case "for loop runs once per word" \
    'for x in a b c; do echo iter; done' \
    'iter' \
    '[[ $(grep -o iter <<< "$ACTUAL_OUTPUT" | wc -l) -eq 3 ]]'

echo
echo "== Variable expansion =="
export FOO=bar
run_case "\$VAR expands to its environment value" \
    'echo $FOO' \
    '\bbar\b'

run_case "unset \$VAR expands to an empty string" \
    'echo $THIS_VAR_IS_NEVER_SET' \
    '' \
    '! grep -q THIS_VAR_IS_NEVER_SET <<< "$ACTUAL_OUTPUT"'

run_case "single quotes suppress expansion" \
    "echo '\$FOO'" \
    '\$FOO'

run_case "the for-loop variable is visible as \$VAR in the body" \
    'for x in one two three; do echo $x; done' \
    'one' \
    'grep -q two <<< "$ACTUAL_OUTPUT" && grep -q three <<< "$ACTUAL_OUTPUT"'

export TARGET_FILE=varfile
rm -f "$WORKDIR/varfile"
run_case "a variable can be used as a redirection target" \
    'echo via_var > $TARGET_FILE' \
    '' \
    'grep -q via_var varfile'

echo
echo "== Double quotes and word concatenation =="
run_case "double quotes still expand \$VAR" \
    'echo "hello $FOO"' \
    'hello bar'

run_case "backslash-escaped \$ inside double quotes is literal" \
    'echo "no expand: \$FOO"' \
    '' \
    '! grep -q "no expand: bar" <<< "$ACTUAL_OUTPUT" && grep -q "no expand: \$FOO" <<< "$ACTUAL_OUTPUT"'

run_case "adjacent bare/quoted/variable parts concatenate into one word" \
    'echo pre$FOO"mid"post' \
    'prebarmidpost'

run_case "an empty quoted string is a distinct empty argument" \
    'echo a "" b' \
    '\ba +b\b'

run_case "a quoted keyword is a literal word, not a keyword" \
    '"if" arg1' \
    "Failed to start command 'if'"

echo
echo "== Remaining punctuation =="
run_case "common punctuation lexes as literal word text" \
    'echo a@b.com 50%off a,b,c user:pass a*b?c' \
    'a@b\.com 50%off a,b,c user:pass a\*b\?c'

touch "$WORKDIR/testfile"
run_case "[ ... ] invokes the real test binary" \
    '[ -f testfile ] && echo test_worked' \
    'test_worked'

run_case "an unquoted backslash escapes the next character, keeping the word open" \
    'echo hello\ world' \
    'hello world$'

run_case "an unquoted backslash-escaped operator is literal" \
    'echo escaped\;semicolon' \
    'escaped;semicolon$'

run_case "an unquoted backslash-escaped \$ suppresses expansion" \
    'echo escaped\$FOO' \
    'escaped\$FOO$'

echo
echo "== Builtins =="
run_case "pwd prints the current directory" \
    'pwd' \
    "$(printf '%s' "$WORKDIR" | sed 's/[.[\*^$]/\\&/g')"

mkdir -p "$WORKDIR/subdir"
run_case "cd changes directory for subsequent commands" \
    'cd subdir
pwd' \
    'subdir$'

run_case "cd .. goes back up" \
    'cd subdir
cd ..
pwd' \
    "$(printf '%s' "$WORKDIR" | sed 's/[.[\*^$]/\\&/g')\$"

run_case "cd with no args goes to HOME" \
    'cd
pwd' \
    "$(printf '%s' "$HOME" | sed 's/[.[\*^$]/\\&/g')\$"

run_case "cd into a missing directory reports an error" \
    'cd this_directory_does_not_exist' \
    'cd:'

run_case "export sets a variable the shell can read back" \
    'export MYVAR=hello
echo $MYVAR' \
    '\bhello\b'

run_case "unset removes a variable" \
    'export MYVAR=hello
unset MYVAR
echo $MYVAR' \
    '' \
    '! grep -q hello <<< "$ACTUAL_OUTPUT"'

rm -f "$WORKDIR/exports.txt"
run_case "export with no args lists variables, honoring redirection" \
    'export ZZZTEST=marker
export > exports.txt' \
    '' \
    'grep -q "ZZZTEST=marker" exports.txt'

run_case "exit stops the shell with the given status code" \
    'echo before
exit 7
echo should_not_run' \
    '' \
    '[[ $ACTUAL_EXIT_CODE -eq 7 ]] && ! grep -q should_not_run <<< "$ACTUAL_OUTPUT"'

echo
echo "== Builtins inside pipelines (forked subshell) =="
run_case "a builtin as the last stage of a pipeline runs and its output is inherited" \
    'echo ignored | pwd' \
    "$(printf '%s' "$WORKDIR" | sed 's/[.[\*^$]/\\&/g')"

run_case "a builtin in the middle of a pipeline still pipes data through" \
    "echo ignored | pwd | grep -o $(basename "$WORKDIR")" \
    "$(basename "$WORKDIR")"

run_case "cd inside a pipeline does not change the parent shell's directory" \
    'cd subdir | true
pwd' \
    "$(printf '%s' "$WORKDIR" | sed 's/[.[\*^$]/\\&/g')\$"

run_case "export inside a pipeline does not leak into the parent shell" \
    'export LEAKVAR=oops | true
echo LEAKVAR=[$LEAKVAR]' \
    'LEAKVAR=\[\]'

run_case "exit inside a pipeline only kills that stage, not the whole shell" \
    'exit 3 | true
echo still_alive' \
    'still_alive'

echo
echo "== Parameter expansion (\${VAR} and friends) =="
export FOO=bar
run_case "\${VAR} expands like \$VAR" \
    'echo ${FOO}' \
    '\bbar\b'

run_case "\${VAR} disambiguates concatenation from \$VAR" \
    'echo ${FOO}baz' \
    'barbaz'

run_case "\${VAR:-default} uses the value when set" \
    'echo ${FOO:-fallback}' \
    '\bbar\b'

run_case "\${VAR:-default} falls back when unset" \
    'echo ${THIS_IS_UNSET_1:-fallback}' \
    'fallback'

run_case "\${VAR:=default} assigns the default so later reads see it" \
    'echo ${THIS_IS_UNSET_2:=assigned}
echo $THIS_IS_UNSET_2' \
    '' \
    '[[ $(grep -c assigned <<< "$ACTUAL_OUTPUT") -eq 2 ]]'

run_case "\${VAR:+alt} is empty when unset" \
    'echo [${THIS_IS_UNSET_3:+alt}]' \
    '\[\]'

run_case "\${VAR:+alt} uses alt when set" \
    'echo [${FOO:+alt}]' \
    '\[alt\]'

run_case "\$? reflects the last command's exit status" \
    'true
echo status_a=$?
false
echo status_b=$?' \
    'status_a=0' \
    'grep -q status_b=1 <<< "$ACTUAL_OUTPUT"'

run_case "\$\$ expands to the shell's own pid" \
    'echo $$' \
    ' [0-9]+$'

echo
echo "-----------------------------------"
echo "Results: ${GREEN}${PASS} passed${NC}, ${RED}${FAIL} failed${NC}"

[[ $FAIL -eq 0 ]]

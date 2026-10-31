# MiniShell (Rust)

A small Unix shell implemented in Rust, inspired by the 42 School `minishell` project. It reads commands from an interactive prompt, parses them into an AST, and executes them.

## Features

### Working
- Interactive REPL (prompt, read, execute, loop; exits cleanly on `Ctrl-D`). On a terminal, line editing and history come from [rustyline](https://crates.io/crates/rustyline): arrow keys, `Ctrl-R` search, and a history persisted in `~/.minishell_history`; heredoc bodies get a `> ` continuation prompt. When stdin is not a terminal (pipe, script, tests), lines are read raw from stdin instead.
- Signals, like an interactive bash: `Ctrl-\` (SIGQUIT) is ignored by the shell; `Ctrl-C` (SIGINT) never kills the shell — at the prompt it drops the current input, during execution it kills the foreground command and abandons the rest of the line (remaining `;` commands, loops), and inside a heredoc it cancels the whole line. Commands started by the shell get the default SIGINT/SIGQUIT behavior back. A command killed by a signal sets `$?` to 128 + the signal number (130 for `Ctrl-C`).
- Lexer: words, single-quoted strings (`'...'`, never expanded), double-quoted strings (`"..."`, `$VAR` still expands inside, `\"`/`\\`/`\$` are recognized escapes), variables (`$VAR`), keywords (`if`/`then`/`elif`/`else`/`fi`, `while`/`until`/`do`/`done`, `for`/`in`) — only recognized when unquoted, logical operators (`&&`, `||`), pipe (`|`), separators (`;`, newline), redirection operators (`>`, `>>`, `>&`, `>|`, `<`, `<&`, `<>`), parentheses, and braces. Adjacent bare/quoted/variable segments with no whitespace between them (e.g. `pre"mid"$VARpost`) are lexed as a single word.
- Parser: simple commands, redirections (`>`, `>|`, `>>`, `<`, `<>`, `>&N`/`<&N`, `>&-`/`<&-`, heredocs `<<`/`<<-`, each optionally prefixed by a file descriptor number like `2>` or `3<`, in any position among the words), pipelines (`|`), `&&` / `||` chains, command lists separated by `;` (on the main line and inside every `if`/`while`/`until`/`for` condition or body; `&&`/`||` bind tighter than `;`, and a trailing `;` is fine), `if` / `elif` / `else` / `fi`, `while` / `until` / `do` / `done`, `for VAR in words... do ... done`. Command names, arguments, and redirection targets can each be a literal word, a single-quoted string, or a `$VAR` reference.
- Parse errors: anything left on the line that the grammar can't place (e.g. a stray `)`) is reported as a `Parse Error` and nothing on that line runs, instead of being silently dropped.
- Executor: simple commands (spawned via `std::process::Command`), redirections (real file I/O via `dup2`, applied left to right so `> f 2>&1` and `2>&1 > f` differ like in bash, and on top of pipes when both are present; builtins get them applied in the shell process itself, then the original fds are restored), pipelines (real OS pipes between child processes), `&&` / `||` short-circuiting, `if`/`elif`/`else`, `while`/`until` loops, `for` loops, and expansion (resolved at execution time; single quotes suppress it; an unset variable expands to an empty string). The `for` loop variable is set as a real environment variable on each iteration so `$VAR` inside the body picks up the current value.
- Parameter expansion: `$VAR` and `${VAR}` (the braced form disambiguates concatenation, e.g. `${VAR}text` vs `$VARtext`), `${VAR:-default}` (use default if unset/empty), `${VAR:=default}` (same, and also assigns it), `${VAR:+alt}` (use alt only if VAR is set and non-empty). Two special parameters: `$?` (exit status of the last command — tracked across simple commands, builtins, and pipelines) and `$$` (the shell's own PID).
- Builtins: `cd [dir]` (defaults to `$HOME`), `pwd`, `export [NAME=value ...]` (no-arg form lists the environment), `unset NAME ...`, `exit [code]`. All honor every redirection form, including `2>` for their own error messages. As the whole command, a builtin runs directly in the shell's own process (so `cd`/`export`/`unset` actually affect the shell, and `exit` actually ends it). As one stage of a multi-stage pipeline, it runs in a real forked child instead — matching real shells, this means `cd`/`export`/`unset`/`exit` there only affect that subshell's copy of the state and vanish once it exits (e.g. `cd /tmp | true` does not change the shell's own directory).
- Lexer words accept common punctuation (`.`, `/`, `-`, `=` once a word has started, `[`, `]`, `#`, `%`, `^`, `*`, `+`, `~`, `@`, `:`, `,`, `?`, `` ` ``) as literal text, so paths (`..`, `/tmp/file`), flags (`-la`), assignments (`NAME=value`), `[ -f file ]`-style test invocations, and things like `user@host` or `a,b,c` all tokenize as expected — none of these characters carry special shell behavior (no globbing for `*`/`?`, no substitution for `` ` ``, no comments for `#`), they just won't break the lexer.
- An unquoted backslash escapes the very next character literally (including a space or an operator like `;`), keeping the word open, the same way real shells do.
- Heredocs: `<<DELIM` reads the following input lines up to a line that is exactly `DELIM`; `$VAR` expands in the body unless the delimiter is quoted (`<<'EOF'`, `<<"EOF"`, `<<\EOF`); `<<-` strips leading tabs from the body and the delimiter lines. The body goes through an unlinked temp file, so a big body can't deadlock a pipe.

### Not yet implemented
- Pipelines of compound commands (e.g. `if ... fi | grep x`) — only simple commands can appear in a pipeline for now
- `&>file` / `>&file` (both streams to a file — use `> file 2>&1`), and `noclobber` (so `>|` behaves exactly like `>`)
- Other parameter forms: `${VAR:?message}`/`${VAR?}` (error if unset), non-colon variants (`${VAR-default}` etc., which treat "set but empty" differently from "unset"), `$#`, `$0`/`$1`/..., `$@`/`$*` — none apply here since this shell has no script arguments/positional parameters
- Word splitting and globbing (`*`, `?`) on expanded values
- Other builtins (`echo` is external for now, so no `-n`/`-e` flags; no `env`, `type`, `history`, ...)

## Project layout

```
src/
├── main.rs           # REPL: prompt/line editing, read line, lex, parse, execute
├── signals.rs        # SIGINT/SIGQUIT dispositions and the "interrupted" flag
├── lexer/
│   ├── lexer.rs       # Lexer: turns input text into a stream of tokens
│   ├── token.rs       # Token and WordPart enums
│   └── mod.rs
├── parser/
│   ├── parser.rs      # Recursive-descent parser: tokens -> AST
│   ├── ast.rs         # AST types: Command, SimpleCommand, LogicalOp, Redirect, Word
│   └── mod.rs
└── executor.rs         # Walks the AST and runs commands via std::process
```

## Building and running

Requires a Rust toolchain (2024 edition).

```bash
cargo build
cargo run
```

This drops you into the `minishell$` prompt:

```
minishell$ echo hello
hello
minishell$ ls && echo ok
...
ok
minishell$ if ls; then echo Success fi
...
Success
```

Exit with `Ctrl-D`.

## Testing

A black-box test suite lives in `tests/run_tests.sh`. It builds the project, then pipes small shell scripts into the binary and checks the resulting output and/or filesystem state (each test runs in its own temp directory).

```bash
bash tests/run_tests.sh
```

It covers simple commands, `&&`/`||`, `;` lists, signals (SIGINT/SIGQUIT handling, 128+n exit statuses), pipelines, redirections (`>`, `>>`, `<`, `>|`, `<>`, `2>`, `2>&1`, `>&2`, `<&3`, `2>&-`), heredocs (`<<`, `<<-`, quoted delimiters), `if`/`elif`/`else`, `while`/`until`, `for`, variable expansion, double quotes and word concatenation, builtins (standalone and as forked pipeline stages), lexer punctuation/escaping, and parameter expansion (`${VAR}`, `:-`/`:=`/`:+`, `$?`, `$$`).

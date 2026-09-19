#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    If,
    Then,
    Elif,
    Else,
    Fi,
    While,
    Until,
    Do,
    Done,
    Semicolon,
    Newline,
    And,
    Or,
    For,
    In,
    Neg,
    Pipeline,

    /// A word, possibly made of several parts glued together with no
    /// whitespace in between (e.g. `pre"mid"$VARpost` is one `Word` with
    /// three parts).
    Word(Vec<WordPart>),

    Assign,

    RedirectOut,
    RedirectIn,
    RedirectAppend,
    RedirectOutErr,
    RedirectInErr,
    RedirectClobber,
    RedirectInOut,

    ParOpen,
    ParClose,
    BraceOpen,
    BraceClose,

    Eof,
    Error(String),
}

/// One piece of a word: literal text (from bare characters or quotes), a
/// `$NAME` reference, or a braced `${NAME...}` parameter expansion.
/// Single-quoted text is always a `Literal`; inside double quotes, everything
/// is `Literal` except for `$NAME`/`${...}`, which still expand.
#[derive(Debug, Clone, PartialEq)]
pub enum WordPart {
    Literal(String),
    Variable(String),
    /// `${name}` (op: None) or `${name:-default}` / `${name:=default}` /
    /// `${name:+default}`. `default` is itself a sequence of parts, since it
    /// may contain further `$NAME`/`${...}` references.
    Parameter {
        name: String,
        op: Option<ParamOp>,
        default: Vec<WordPart>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamOp {
    /// `${name:-default}`: use `default` if `name` is unset or empty.
    UseDefault,
    /// `${name:=default}`: like `UseDefault`, and also assigns `default` to `name`.
    AssignDefault,
    /// `${name:+default}`: use `default` if `name` is set and non-empty, else empty.
    UseAlternative,
}

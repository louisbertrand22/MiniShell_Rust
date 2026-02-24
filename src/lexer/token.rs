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

    Word(String),
    SqWord(String),
    Var(String),
    IoNumber(u32),

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
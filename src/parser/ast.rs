#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Simple(SimpleCommand),
    Pipeline(Vec<Command>),
    BinaryOp {
        left: Box<Command>,
        op: LogicalOp,
        right: Box<Command>,
    },
    If {
        condition: Box<Command>,
        then_part: Box<Command>,
        else_part: Option<Box<Command>>,
    },
    /// `while condition; do body; done` when `until` is false,
    /// `until condition; do body; done` when `until` is true.
    Loop {
        condition: Box<Command>,
        body: Box<Command>,
        until: bool,
    },
    For {
        var: String,
        words: Vec<Word>,
        body: Box<Command>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogicalOp {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimpleCommand {
    pub name: Word,
    pub args: Vec<Word>,
    pub redirects: Vec<Redirect>,
}

/// A word that still needs to be expanded before it can be used: a plain
/// literal, a `$NAME` reference to be substituted with an environment
/// variable's value at execution time, a `${NAME...}` parameter expansion,
/// or several such parts glued together with no whitespace between them
/// (e.g. `pre"mid"$VARpost`). Text inside single quotes is always a
/// `Literal`, since single quotes suppress expansion; double quotes still
/// expand `$NAME`/`${...}` inside them.
#[derive(Debug, Clone, PartialEq)]
pub enum Word {
    Literal(String),
    Variable(String),
    Concat(Vec<Word>),
    Parameter {
        name: String,
        op: Option<ParamOp>,
        default: Box<Word>,
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

#[derive(Debug, Clone, PartialEq)]
pub enum Redirect {
    Output(Word),
    Input(Word),
    Append(Word),
}
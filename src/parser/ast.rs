#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Simple(SimpleCommand),
    /// `cmd1; cmd2; ...`: exécutées l'une après l'autre, quel que soit leur
    /// résultat; le statut de la liste est celui de la dernière.
    Sequence(Vec<Command>),
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

/// Une redirection appliquée au fd `fd` (0 pour les formes d'entrée, 1 pour
/// les formes de sortie, sauf numéro explicite comme dans `2>file`).
#[derive(Debug, Clone, PartialEq)]
pub struct Redirect {
    pub fd: i32,
    pub kind: RedirectKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RedirectKind {
    /// `>` et `>|` (pas d'option noclobber dans ce shell, donc identiques).
    Output(Word),
    /// `>>`
    Append(Word),
    /// `<`
    Input(Word),
    /// `<>`: ouvre le fichier en lecture/écriture, sans le tronquer.
    ReadWrite(Word),
    /// `>&N` / `<&N` (duplique le fd N) ou `>&-` / `<&-` (ferme le fd).
    Dup(Word),
    /// `<<` / `<<-`: le corps du heredoc, déjà lu. Un `Word` pour que `$VAR`
    /// y soit développé à l'exécution (sauf délimiteur quoté: tout littéral).
    Heredoc(Word),
}
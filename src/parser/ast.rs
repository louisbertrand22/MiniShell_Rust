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
    // Add While, For, etc. as needed
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogicalOp {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimpleCommand {
    pub name: String,
    pub args: Vec<String>,
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Redirect {
    Output(String),
    Input(String),
    Append(String),
}
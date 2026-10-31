mod lexer;
mod parser;
mod executor;
mod signals;

use std::cell::RefCell;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::rc::Rc;

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use lexer::Lexer;
use parser::Parser;
use executor::Executor;

const PROMPT: &str = "minishell$ ";
const CONTINUATION_PROMPT: &str = "> ";

enum ReadResult {
    Line(String),
    /// Ctrl-C pendant la saisie (reçu comme une touche par l'éditeur).
    Interrupted,
    Eof,
}

/// D'où viennent les lignes: un éditeur de ligne (historique, flèches) sur un
/// terminal, ou une lecture brute de stdin sinon (script, pipe, tests).
enum Input {
    Interactive { editor: Box<DefaultEditor>, history: Option<PathBuf> },
    Piped,
}

impl Input {
    fn new() -> Self {
        if !io::stdin().is_terminal() {
            return Input::Piped;
        }
        match DefaultEditor::new() {
            Ok(mut editor) => {
                let history = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".minishell_history"));
                if let Some(path) = &history {
                    editor.load_history(path).ok(); // absent au premier lancement
                }
                Input::Interactive { editor: Box::new(editor), history }
            }
            Err(_) => Input::Piped,
        }
    }

    fn is_interactive(&self) -> bool {
        matches!(self, Input::Interactive { .. })
    }

    /// Lit une ligne (sans son `\n`). En mode pipe, le prompt principal est
    /// quand même affiché, mais pas celui de continuation.
    fn read(&mut self, prompt: &str, continuation: bool) -> ReadResult {
        match self {
            Input::Interactive { editor, .. } => match editor.readline(prompt) {
                Ok(line) => ReadResult::Line(line),
                Err(ReadlineError::Interrupted) => ReadResult::Interrupted,
                Err(_) => ReadResult::Eof,
            },
            Input::Piped => {
                if !continuation {
                    print!("{prompt}");
                    io::stdout().flush().ok();
                }
                let mut line = String::new();
                match io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => ReadResult::Eof,
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                        }
                        ReadResult::Line(line)
                    }
                }
            }
        }
    }

    fn remember(&mut self, line: &str) {
        if let Input::Interactive { editor, history } = self {
            editor.add_history_entry(line).ok();
            if let Some(path) = history {
                editor.save_history(path).ok();
            }
        }
    }
}

fn main() {
    signals::install();
    let input = Rc::new(RefCell::new(Input::new()));
    let interactive = input.borrow().is_interactive();

    loop {
        signals::clear();

        let line = match input.borrow_mut().read(PROMPT, false) {
            ReadResult::Line(line) => line,
            ReadResult::Interrupted => {
                // Ctrl-C au prompt: on abandonne la saisie, comme bash ($? = 130).
                Executor::set_last_status(130);
                continue;
            }
            ReadResult::Eof => {
                // Ctrl-D
                println!();
                break;
            }
        };

        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        input.borrow_mut().remember(line);

        let lexer = Lexer::new(line);
        // Les corps de heredoc sont lus sur les lignes qui suivent celle-ci.
        let source = Rc::clone(&input);
        let mut parser = Parser::with_line_source(
            lexer,
            Box::new(move || match source.borrow_mut().read(CONTINUATION_PROMPT, true) {
                ReadResult::Line(line) => Some(line),
                ReadResult::Interrupted => {
                    // Ctrl-C dans un heredoc: toute la ligne est abandonnée.
                    signals::set_interrupted();
                    None
                }
                ReadResult::Eof => None,
            }),
        );

        match parser.parse() {
            Ok(ast) => {
                // Une erreur vide a déjà été affichée par l'executor lui-même.
                if let Err(e) = Executor::execute(ast)
                    && !e.is_empty()
                {
                    eprintln!("Execution Error: {e}");
                }
            }
            Err(e) => eprintln!("Parse Error: {e}"),
        }

        // Le terminal a affiché `^C` sans retour à la ligne: le prompt suivant
        // repart sur une ligne propre.
        if interactive && signals::interrupted() {
            println!();
        }
    }
}

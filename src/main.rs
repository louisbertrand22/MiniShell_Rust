mod lexer;
mod parser;
mod executor; //

use std::io::{self, Write};

use lexer::Lexer;
use parser::Parser;
use executor::Executor;

fn main() {
    loop {
        print!("minishell$ ");
        io::stdout().flush().ok();

        let mut input = String::new();
        let bytes_read = io::stdin().read_line(&mut input).unwrap_or(0);

        if bytes_read == 0 {
            // EOF (Ctrl-D)
            println!();
            break;
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        let lexer = Lexer::new(input);
        // Les corps de heredoc sont lus sur les lignes qui suivent celle-ci.
        let mut parser = Parser::with_line_source(lexer, Box::new(read_continuation_line));

        match parser.parse() {
            Ok(ast) => {
                // Une erreur vide a déjà été affichée par l'executor lui-même.
                if let Err(e) = Executor::execute(ast)
                    && !e.is_empty()
                {
                    eprintln!("Execution Error: {}", e);
                }
            }
            Err(e) => eprintln!("Parse Error: {}", e),
        }
    }
}
/// Lit une ligne de plus sur stdin (sans son `\n`), ou `None` en fin d'entrée.
fn read_continuation_line() -> Option<String> {
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => {
            if line.ends_with('\n') {
                line.pop();
            }
            Some(line)
        }
    }
}

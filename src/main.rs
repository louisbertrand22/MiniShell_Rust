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
        let mut parser = Parser::new(lexer);

        match parser.parse() {
            Ok(ast) => {
                if let Err(e) = Executor::execute(ast) {
                    eprintln!("Execution Error: {}", e);
                }
            }
            Err(e) => eprintln!("Parse Error: {}", e),
        }
    }
}
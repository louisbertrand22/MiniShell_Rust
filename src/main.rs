mod lexer;
mod parser;

use lexer::{Lexer, Token};
use parser::Parser;

fn main() {
    let mut lexer = Lexer::new("if test; then echo hello fi");
    let mut parser = Parser::new(lexer);

    match parser.parse() {
        Ok(ast) => println!("Parsed AST: {:#?}", ast),
        Err(e) => eprintln!("Error: {}", e),
    }
}
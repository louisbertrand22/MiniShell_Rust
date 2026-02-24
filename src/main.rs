mod lexer;
mod parser;
mod executor; //

use lexer::Lexer;
use parser::Parser;
use executor::Executor;

fn main() {
    // Note: 'test' is a common shell command used for conditions
    let input = "if ls && ls; then echo Success fi"; 
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);

    match parser.parse() {
        Ok(ast) => {
            println!("Executing AST...");
            if let Err(e) = Executor::execute(ast) {
                eprintln!("Execution Error: {}", e);
            }
        }
        Err(e) => eprintln!("Parse Error: {}", e),
    }
}
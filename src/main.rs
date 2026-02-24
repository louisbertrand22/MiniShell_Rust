mod lexer;

use lexer::{Lexer, Token};

fn main() {
    let mut lexer = Lexer::new("if x > 10 || true; then echo hello");

    loop {
        let tok = lexer.next_token();
        println!("{:?}", tok);

        if tok == Token::Eof {
            break;
        }
    }
}
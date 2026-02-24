use crate::lexer::{Lexer, Token};
use crate::parser::ast::{Command, SimpleCommand};

pub struct Parser {
    lexer: Lexer,
    current_token: Token,
}

impl Parser {
    pub fn new(mut lexer: Lexer) -> Self {
        let current_token = lexer.next_token();
        Parser { lexer, current_token }
    }

    fn advance(&mut self) {
        self.current_token = self.lexer.next_token();
    }

    fn expect(&mut self, expected: Token) -> Result<(), String> {
        if self.current_token == expected {
            self.advance();
            Ok(())
        } else {
            Err(format!("Expected {:?}, found {:?}", expected, self.current_token))
        }
    }

    pub fn parse(&mut self) -> Result<Command, String> {
        match self.current_token {
            Token::If => self.parse_if(),
            Token::Word(_) => Ok(Command::Simple(self.parse_simple_command()?)),
            _ => Err(format!("Unexpected token: {:?}", self.current_token)),
        }
    }

    fn parse_simple_command(&mut self) -> Result<SimpleCommand, String> {
        let mut name = String::new();
        let mut args = Vec::new();

        if let Token::Word(s) = &self.current_token {
            name = s.clone();
            self.advance();
        }

        while let Token::Word(s) = &self.current_token {
            args.push(s.clone());
            self.advance();
        }

        Ok(SimpleCommand {
            name,
            args,
            redirects: Vec::new(),
        })
    }

    fn parse_if(&mut self) -> Result<Command, String> {
        self.advance(); // consume 'if'
        let condition = Box::new(self.parse()?);
        
        // Basic handling for semicolon or newline after condition
        if self.current_token == Token::Semicolon { self.advance(); }
        
        self.expect(Token::Then)?;
        let then_part = Box::new(self.parse()?);
        
        self.expect(Token::Fi)?;

        Ok(Command::If {
            condition,
            then_part,
            else_part: None,
        })
    }
}
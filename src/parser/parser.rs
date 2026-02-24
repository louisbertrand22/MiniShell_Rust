use crate::lexer::{Lexer, Token};
use crate::parser::ast::{Command, SimpleCommand, LogicalOp};

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
        self.parse_expression()
    }

    fn parse_expression(&mut self) -> Result<Command, String> {
        let mut left = self.parse_command()?;

        loop {
            let op = match self.current_token {
                Token::And => LogicalOp::And,
                Token::Or => LogicalOp::Or,
                _ => break,
            };

            self.advance(); // Consomme && ou ||
            let right = self.parse_command()?;
            
            left = Command::BinaryOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    /// Gère les commandes de base (if, simple commands, subshells)
    fn parse_command(&mut self) -> Result<Command, String> {
        match self.current_token {
            Token::If => self.parse_if(),
            Token::Word(_) => Ok(Command::Simple(self.parse_simple_command()?)),
            _ => Err(format!("Unexpected token in command: {:?}", self.current_token)),
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
        // On utilise parse_expression ici pour autoriser "if cmd1 && cmd2; then ..."
        let condition = Box::new(self.parse_expression()?);
        
        if self.current_token == Token::Semicolon || self.current_token == Token::Newline {
            self.advance();
        }
        
        self.expect(Token::Then)?;
        let then_part = Box::new(self.parse_expression()?);
        
        // Gestion optionnelle du else
        let mut else_part = None;
        if self.current_token == Token::Else {
            self.advance();
            else_part = Some(Box::new(self.parse_expression()?));
        }

        self.expect(Token::Fi)?;

        Ok(Command::If {
            condition,
            then_part,
            else_part,
        })
    }
}
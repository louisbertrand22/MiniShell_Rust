use crate::lexer::{Lexer, ParamOp as LexParamOp, Token, WordPart};
use crate::parser::ast::{Command, SimpleCommand, LogicalOp, ParamOp, Redirect, Word};

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
        let mut left = self.parse_pipeline()?;

        loop {
            let op = match self.current_token {
                Token::And => LogicalOp::And,
                Token::Or => LogicalOp::Or,
                _ => break,
            };

            self.advance(); // Consomme && ou ||
            let right = self.parse_pipeline()?;

            left = Command::BinaryOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    /// Gère les pipelines: command ( '|' command )*
    fn parse_pipeline(&mut self) -> Result<Command, String> {
        let mut commands = vec![self.parse_command()?];

        while self.current_token == Token::Pipeline {
            self.advance(); // Consomme '|'
            commands.push(self.parse_command()?);
        }

        if commands.len() == 1 {
            Ok(commands.pop().unwrap())
        } else {
            Ok(Command::Pipeline(commands))
        }
    }

    /// Gère les commandes de base (if, while, until, for, simple commands, subshells)
    fn parse_command(&mut self) -> Result<Command, String> {
        match self.current_token {
            Token::If => self.parse_if(),
            Token::While => self.parse_loop(false),
            Token::Until => self.parse_loop(true),
            Token::For => self.parse_for(),
            Token::Word(_) => Ok(Command::Simple(self.parse_simple_command()?)),
            _ => Err(format!("Unexpected token in command: {:?}", self.current_token)),
        }
    }

    /// Consomme un séparateur de liste optionnel (';' ou newline)
    fn skip_separator(&mut self) {
        if self.current_token == Token::Semicolon || self.current_token == Token::Newline {
            self.advance();
        }
    }

    /// Consomme le token courant s'il forme un mot, sans avancer sinon.
    fn take_word(&mut self) -> Option<Word> {
        if !matches!(self.current_token, Token::Word(_)) {
            return None;
        }

        let token = std::mem::replace(&mut self.current_token, Token::Eof);
        self.advance();

        match token {
            Token::Word(parts) => Some(Self::word_from_parts(parts)),
            _ => unreachable!("guarded by the matches! check above"),
        }
    }

    /// Convertit les segments produits par le lexer en `ast::Word`: un seul
    /// segment devient directement ce segment, plusieurs deviennent un `Concat`.
    fn word_from_parts(parts: Vec<WordPart>) -> Word {
        let mut words: Vec<Word> = parts.into_iter().map(Self::word_from_part).collect();

        match words.len() {
            0 => Word::Literal(String::new()),
            1 => words.pop().unwrap(),
            _ => Word::Concat(words),
        }
    }

    fn word_from_part(part: WordPart) -> Word {
        match part {
            WordPart::Literal(s) => Word::Literal(s),
            WordPart::Variable(name) => Word::Variable(name),
            WordPart::Parameter { name, op, default } => Word::Parameter {
                name,
                op: op.map(Self::convert_param_op),
                default: Box::new(Self::word_from_parts(default)),
            },
        }
    }

    fn convert_param_op(op: LexParamOp) -> ParamOp {
        match op {
            LexParamOp::UseDefault => ParamOp::UseDefault,
            LexParamOp::AssignDefault => ParamOp::AssignDefault,
            LexParamOp::UseAlternative => ParamOp::UseAlternative,
        }
    }

    fn parse_simple_command(&mut self) -> Result<SimpleCommand, String> {
        let mut name = Word::Literal(String::new());
        let mut name_set = false;
        let mut args = Vec::new();
        let mut redirects = Vec::new();

        loop {
            if let Some(word) = self.take_word() {
                if name_set {
                    args.push(word);
                } else {
                    name = word;
                    name_set = true;
                }
                continue;
            }

            match self.current_token {
                Token::RedirectOut | Token::RedirectAppend | Token::RedirectIn => {
                    redirects.push(self.parse_redirect()?);
                }
                _ => break,
            }
        }

        Ok(SimpleCommand {
            name,
            args,
            redirects,
        })
    }

    /// Gère un opérateur de redirection suivi de sa cible (fichier)
    fn parse_redirect(&mut self) -> Result<Redirect, String> {
        let make: fn(Word) -> Redirect = match self.current_token {
            Token::RedirectOut => Redirect::Output,
            Token::RedirectAppend => Redirect::Append,
            Token::RedirectIn => Redirect::Input,
            _ => unreachable!("parse_redirect called on a non-redirect token"),
        };
        self.advance(); // Consomme l'opérateur

        let target = self
            .take_word()
            .ok_or_else(|| format!("Expected a filename after redirection, found {:?}", self.current_token))?;

        Ok(make(target))
    }

    fn parse_if(&mut self) -> Result<Command, String> {
        self.advance(); // consume 'if'

        // On utilise parse_expression ici pour autoriser "if cmd1 && cmd2; then ..."
        let condition = Box::new(self.parse_expression()?);
        self.skip_separator();
        self.expect(Token::Then)?;
        let then_part = Box::new(self.parse_expression()?);
        self.skip_separator();

        // Chaque branche if/elif est empilée, puis repliée en Command::If imbriqués
        let mut branches = vec![(condition, then_part)];

        while self.current_token == Token::Elif {
            self.advance(); // consume 'elif'
            let elif_condition = Box::new(self.parse_expression()?);
            self.skip_separator();
            self.expect(Token::Then)?;
            let elif_then = Box::new(self.parse_expression()?);
            self.skip_separator();
            branches.push((elif_condition, elif_then));
        }

        // Gestion optionnelle du else
        let mut else_part = None;
        if self.current_token == Token::Else {
            self.advance();
            else_part = Some(Box::new(self.parse_expression()?));
            self.skip_separator();
        }

        self.expect(Token::Fi)?;

        let mut result = else_part;
        for (condition, then_part) in branches.into_iter().rev() {
            result = Some(Box::new(Command::If {
                condition,
                then_part,
                else_part: result,
            }));
        }

        Ok(*result.unwrap())
    }

    fn parse_loop(&mut self, until: bool) -> Result<Command, String> {
        self.advance(); // consume 'while' or 'until'

        let condition = Box::new(self.parse_expression()?);
        self.skip_separator();
        self.expect(Token::Do)?;
        let body = Box::new(self.parse_expression()?);
        self.skip_separator();
        self.expect(Token::Done)?;

        Ok(Command::Loop {
            condition,
            body,
            until,
        })
    }

    fn parse_for(&mut self) -> Result<Command, String> {
        self.advance(); // consume 'for'

        let var = match self.take_word() {
            Some(Word::Literal(s)) => s,
            Some(_) => {
                return Err("Expected a plain variable name after 'for', not a quote or expansion".to_string())
            }
            None => return Err(format!("Expected a variable name after 'for', found {:?}", self.current_token)),
        };

        self.expect(Token::In)?;

        let mut words = Vec::new();
        while let Some(word) = self.take_word() {
            words.push(word);
        }

        self.skip_separator();
        self.expect(Token::Do)?;
        let body = Box::new(self.parse_expression()?);
        self.skip_separator();
        self.expect(Token::Done)?;

        Ok(Command::For { var, words, body })
    }
}
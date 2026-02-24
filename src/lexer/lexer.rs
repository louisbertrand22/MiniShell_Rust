use super::token::Token;

pub struct Lexer {
    input: Vec<char>,
    position: usize,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            position: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.position).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.input.get(self.position + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek();
        if ch.is_some() {
            self.position += 1;
        }
        ch
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\r')) {
            self.advance();
        }
    }

    fn read_while<F>(&mut self, condition: F) -> String
    where
        F: Fn(char) -> bool,
    {
        let mut result = String::new();

        while let Some(c) = self.peek() {
            if condition(c) {
                result.push(c);
                self.advance();
            } else {
                break;
            }
        }

        result
    }

    fn read_single_quoted(&mut self) -> Token {
        // on est déjà après le '
        let content = self.read_while(|c| c != '\'');
        self.advance(); // consomme la quote fermante
        Token::SqWord(content)
    }

    fn read_variable(&mut self) -> Token {
        let name = self.read_while(|c| c.is_alphanumeric() || c == '_');
        Token::Var(name)
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_whitespace();

        match self.advance() {
            Some(';') => Token::Semicolon,
            Some('\n') => Token::Newline,

            Some('&') => {
                if self.peek() == Some('&') {
                    self.advance();
                    Token::And
                } else {
                    Token::And
                }
            }

            Some('|') => {
                if self.peek() == Some('|') {
                    self.advance();
                    Token::Or
                } else {
                    Token::Pipeline
                }
            }

            Some('!') => Token::Neg,
            Some('=') => Token::Assign,

            Some('>') => {
                match self.peek() {
                    Some('>') => {
                        self.advance();
                        Token::RedirectAppend
                    }
                    Some('&') => {
                        self.advance();
                        Token::RedirectOutErr
                    }
                    Some('|') => {
                        self.advance();
                        Token::RedirectClobber
                    }
                    _ => Token::RedirectOut,
                }
            }

            Some('<') => {
                match self.peek() {
                    Some('&') => {
                        self.advance();
                        Token::RedirectInErr
                    }
                    Some('>') => {
                        self.advance();
                        Token::RedirectInOut
                    }
                    _ => Token::RedirectIn,
                }
            }

            Some('(') => Token::ParOpen,
            Some(')') => Token::ParClose,
            Some('{') => Token::BraceOpen,
            Some('}') => Token::BraceClose,

            Some('\'') => self.read_single_quoted(),

            Some('$') => self.read_variable(),

            Some(c) if c.is_ascii_digit() => {
                let mut number = c.to_string();
                number.push_str(&self.read_while(|c| c.is_ascii_digit()));
                let value = number.parse::<u32>().unwrap_or(0);
                Token::IoNumber(value)
            }

            Some(c) if c.is_alphabetic() || c == '_' => {
                let mut ident = c.to_string();
                ident.push_str(&self.read_while(|c| c.is_alphanumeric() || c == '_'));

                match ident.as_str() {
                    "if" => Token::If,
                    "then" => Token::Then,
                    "elif" => Token::Elif,
                    "else" => Token::Else,
                    "fi" => Token::Fi,
                    "while" => Token::While,
                    "until" => Token::Until,
                    "do" => Token::Do,
                    "done" => Token::Done,
                    "for" => Token::For,
                    "in" => Token::In,
                    _ => Token::Word(ident),
                }
            }

            None => Token::Eof,

            Some(c) => Token::Error(format!("Unexpected character: {}", c)),
        }
    }
}
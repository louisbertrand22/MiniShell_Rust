use super::token::{ParamOp, Token, WordPart};

/// Ponctuation acceptée aussi bien en début qu'en milieu de mot: pas de sens
/// spécial dans ce shell (pas de globbing pour `*`/`?`, pas de substitution
/// pour `` ` ``, pas de commentaires pour `#`, ...), mais ne doit pas non
/// plus faire planter le lexer quand elle apparaît dans un mot ou un chemin
/// (ex. `[`/`]` pour invoquer le binaire `test`/`[`, `user@host`, `a,b`, ...).
fn is_extra_word_punct(c: char) -> bool {
    matches!(
        c,
        '_' | '.' | '/' | '-' | '[' | ']' | '#' | '%' | '^' | '*' | '+' | '~' | '@' | ':' | ',' | '?' | '`' | '=' | '!'
    )
}

/// Caractères pouvant démarrer une portion nue (non quotée) d'un mot.
/// `=` et `!` en font partie: `[ a = b ]`, `[ ! -f x ]`, `NAME=value`.
fn is_word_start(c: char) -> bool {
    c.is_alphanumeric() || is_extra_word_punct(c)
}

/// Caractères pouvant continuer une portion nue déjà démarrée.
fn is_word_continue(c: char) -> bool {
    is_word_start(c)
}

fn keyword_token(word: &str) -> Option<Token> {
    Some(match word {
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
        "!" => Token::Bang,
        _ => return None,
    })
}

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

    fn peek2(&self) -> Option<char> {
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

    fn read_var_name(&mut self) -> String {
        self.read_while(|c| c.is_alphanumeric() || c == '_')
    }

    /// Lit ce qui suit un `$` déjà consommé: `{...}` (paramètre éventuellement
    /// avec opérateur), `?`/`$` (paramètres spéciaux: dernier code de sortie,
    /// PID du shell), ou un nom de variable classique. `None` si rien de
    /// valide ne suit (le `$` doit alors rester un caractère littéral).
    fn read_dollar_part(&mut self) -> Option<WordPart> {
        if self.peek() == Some('{') {
            self.advance(); // consomme '{'
            return Some(self.read_braced_parameter());
        }

        if matches!(self.peek(), Some('?') | Some('$')) {
            let c = self.advance().unwrap();
            return Some(WordPart::Variable(c.to_string()));
        }

        let name = self.read_var_name();
        if name.is_empty() {
            None
        } else {
            Some(WordPart::Variable(name))
        }
    }

    /// Lit l'intérieur de `${...}` (juste après le `{`), jusqu'à la `}`
    /// fermante (ou l'EOF, tolérée comme pour les quotes non terminées).
    fn read_braced_parameter(&mut self) -> WordPart {
        let name = if matches!(self.peek(), Some('?') | Some('$')) {
            self.advance().unwrap().to_string()
        } else {
            self.read_var_name()
        };

        let op = if self.peek() == Some(':') {
            match self.peek2() {
                Some('-') => {
                    self.advance();
                    self.advance();
                    Some(ParamOp::UseDefault)
                }
                Some('=') => {
                    self.advance();
                    self.advance();
                    Some(ParamOp::AssignDefault)
                }
                Some('+') => {
                    self.advance();
                    self.advance();
                    Some(ParamOp::UseAlternative)
                }
                _ => None,
            }
        } else {
            None
        };

        let default = if op.is_some() {
            self.read_default_parts()
        } else {
            Vec::new()
        };

        if self.peek() == Some('}') {
            self.advance();
        }

        WordPart::Parameter { name, op, default }
    }

    /// Lit le mot par défaut d'un `${name:-default}` (et consorts), qui peut
    /// lui-même contenir du texte littéral et des `$NAME`/`${...}`.
    fn read_default_parts(&mut self) -> Vec<WordPart> {
        let mut parts = Vec::new();
        let mut buffer = String::new();

        loop {
            match self.peek() {
                None | Some('}') => break,
                Some('$') => {
                    self.advance();
                    self.push_dollar(&mut buffer, &mut parts);
                }
                Some(c) => {
                    buffer.push(c);
                    self.advance();
                }
            }
        }

        if !buffer.is_empty() {
            parts.push(WordPart::Literal(buffer));
        }

        parts
    }

    /// Insère le résultat d'un `$...` (déjà consommé) dans `parts`/`buffer`:
    /// flush le texte littéral accumulé avant d'ajouter l'expansion, ou
    /// remet un simple `$` littéral dans le buffer si rien ne suivait.
    fn push_dollar(&mut self, buffer: &mut String, parts: &mut Vec<WordPart>) {
        match self.read_dollar_part() {
            Some(part) => {
                if !buffer.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(buffer)));
                }
                parts.push(part);
            }
            None => buffer.push('$'),
        }
    }

    /// Lit un mot complet: une suite contiguë (sans espace) de segments nus,
    /// de chaînes entre quotes simples ou doubles, et de références `$NAME`,
    /// tous fusionnés dans le même token. Un mot-clé (`if`, `while`, ...)
    /// n'est reconnu que si le mot est un unique segment nu, non quoté.
    fn read_word(&mut self) -> Token {
        let mut parts: Vec<WordPart> = Vec::new();
        let mut buffer = String::new();
        let mut quoted = false;

        loop {
            match self.peek() {
                Some('\'') => {
                    quoted = true;
                    self.advance(); // consomme la quote ouvrante
                    buffer.push_str(&self.read_while(|c| c != '\''));
                    self.advance(); // consomme la quote fermante
                }
                Some('"') => {
                    quoted = true;
                    self.advance(); // consomme la quote ouvrante
                    loop {
                        match self.peek() {
                            None | Some('"') => break,
                            Some('\\') => {
                                self.advance();
                                match self.peek() {
                                    Some(c @ ('"' | '\\' | '$')) => {
                                        buffer.push(c);
                                        self.advance();
                                    }
                                    Some(c) => {
                                        buffer.push('\\');
                                        buffer.push(c);
                                        self.advance();
                                    }
                                    None => buffer.push('\\'),
                                }
                            }
                            Some('$') => {
                                self.advance();
                                self.push_dollar(&mut buffer, &mut parts);
                            }
                            Some(c) => {
                                buffer.push(c);
                                self.advance();
                            }
                        }
                    }
                    self.advance(); // consomme la quote fermante
                }
                Some('$') => {
                    self.advance();
                    self.push_dollar(&mut buffer, &mut parts);
                }
                // Backslash hors quotes: échappe littéralement le caractère suivant,
                // quel qu'il soit (espace, opérateur, `$`, ...), et garde le mot ouvert.
                Some('\\') => {
                    quoted = true; // même effet qu'une quote: pas de reconnaissance de mot-clé
                    self.advance();
                    match self.peek() {
                        Some(c) => {
                            buffer.push(c);
                            self.advance();
                        }
                        None => buffer.push('\\'),
                    }
                }
                Some(c) if is_word_start(c) => {
                    buffer.push_str(&self.read_while(is_word_continue));
                }
                _ => break,
            }
        }

        if !buffer.is_empty() {
            parts.push(WordPart::Literal(buffer));
        }

        if !quoted {
            if let [WordPart::Literal(s)] = parts.as_slice() {
                if let Some(keyword) = keyword_token(s) {
                    return keyword;
                }
            }
        }

        Token::Word(parts)
    }

    /// Si la position courante est une suite de chiffres immédiatement suivie
    /// de `<` ou `>` (ex. `2>`), consomme les chiffres et renvoie le numéro de fd.
    fn try_io_number(&mut self) -> Option<Token> {
        let digits = self.input[self.position..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if digits == 0 || !matches!(self.input.get(self.position + digits), Some('<' | '>')) {
            return None;
        }

        let text: String = self.input[self.position..self.position + digits].iter().collect();
        let fd = text.parse().ok()?;
        self.position += digits;
        Some(Token::IoNumber(fd))
    }

    /// Lit le corps brut d'un heredoc: comme dans un double-quote, seul `$`
    /// (et `\$`, `\\`) est spécial; les quotes y sont du texte ordinaire.
    pub fn heredoc_parts(body: &str) -> Vec<WordPart> {
        let mut lexer = Lexer::new(body);
        let mut parts = Vec::new();
        let mut buffer = String::new();

        while let Some(c) = lexer.advance() {
            match c {
                '\\' => match lexer.peek() {
                    Some(next @ ('$' | '\\')) => {
                        buffer.push(next);
                        lexer.advance();
                    }
                    _ => buffer.push('\\'),
                },
                '$' => lexer.push_dollar(&mut buffer, &mut parts),
                c => buffer.push(c),
            }
        }

        if !buffer.is_empty() {
            parts.push(WordPart::Literal(buffer));
        }
        parts
    }

    /// Lit le délimiteur qui suit un `<<`/`<<-` déjà consommé, sans aucune
    /// expansion: les quotes sont retirées, et leur présence (ou celle d'un
    /// backslash) est signalée par le booléen, car elle désactive l'expansion
    /// de `$VAR` dans le corps. `None` si aucun délimiteur ne suit.
    pub fn read_heredoc_delimiter(&mut self) -> Option<(String, bool)> {
        self.skip_whitespace();
        let mut delimiter = String::new();
        let mut quoted = false;
        let mut found = false;

        while let Some(c) = self.peek() {
            match c {
                '\'' | '"' => {
                    quoted = true;
                    found = true;
                    self.advance();
                    delimiter.push_str(&self.read_while(|x| x != c));
                    self.advance(); // quote fermante
                }
                '\\' => {
                    quoted = true;
                    found = true;
                    self.advance();
                    if let Some(next) = self.advance() {
                        delimiter.push(next);
                    }
                }
                ' ' | '\t' | '\r' | '\n' | ';' | '|' | '&' | '<' | '>' | '(' | ')' => break,
                _ => {
                    found = true;
                    delimiter.push(c);
                    self.advance();
                }
            }
        }

        found.then_some((delimiter, quoted))
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_whitespace();

        if let Some(token) = self.try_io_number() {
            return token;
        }

        match self.peek() {
            Some('\'') | Some('"') | Some('$') | Some('\\') => return self.read_word(),
            Some(c) if is_word_start(c) => return self.read_word(),
            _ => {}
        }

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


            Some('>') => {
                match self.peek() {
                    Some('>') => {
                        self.advance();
                        Token::RedirectAppend
                    }
                    Some('&') => {
                        self.advance();
                        Token::RedirectDupOut
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
                        Token::RedirectDupIn
                    }
                    Some('<') => {
                        self.advance();
                        if self.peek() == Some('-') {
                            self.advance();
                            Token::HeredocStrip
                        } else {
                            Token::Heredoc
                        }
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

            None => Token::Eof,

            Some(c) => Token::Error(format!("Unexpected character: {}", c)),
        }
    }
}

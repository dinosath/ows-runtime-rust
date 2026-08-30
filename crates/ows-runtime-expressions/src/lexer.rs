//! Token types and the lexer for the sandboxed expression language.

use super::error::ExpressionError;

/// A lexical token.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// `.`
    Dot,
    /// `..`
    RecursiveDot,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `|`
    Pipe,
    /// `,`
    Comma,
    /// `:`
    Colon,
    /// `;`
    Semicolon,
    /// `$` followed by an identifier
    Variable(String),
    /// An identifier (e.g. function name, `and`, `or`, `true`)
    Ident(String),
    /// A number literal
    Number(f64),
    /// A quoted string literal segment (already de-escaped)
    Str(String),
    /// A `\( source )` interpolation segment inside a string
    Interp(String),
    /// `==`
    EqEq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `//` (alternative operator)
    Alternative,
    /// `=`
    Assign,
    /// End of input
    Eof,
}

impl Token {
    /// A short description used in error messages.
    pub fn describe(&self) -> String {
        match self {
            Token::Dot => ".".into(),
            Token::RecursiveDot => "..".into(),
            Token::LBracket => "[".into(),
            Token::RBracket => "]".into(),
            Token::LBrace => "{".into(),
            Token::RBrace => "}".into(),
            Token::LParen => "(".into(),
            Token::RParen => ")".into(),
            Token::Pipe => "|".into(),
            Token::Comma => ",".into(),
            Token::Colon => ":".into(),
            Token::Semicolon => ";".into(),
            Token::Variable(v) => format!("${v}"),
            Token::Ident(i) => i.clone(),
            Token::Number(n) => format!("{n}"),
            Token::Str(s) => format!("{s:?}"),
            Token::Interp(s) => format!("\\\\({s})"),
            Token::EqEq => "==".into(),
            Token::Ne => "!=".into(),
            Token::Lt => "<".into(),
            Token::Le => "<=".into(),
            Token::Gt => ">".into(),
            Token::Ge => ">=".into(),
            Token::Plus => "+".into(),
            Token::Minus => "-".into(),
            Token::Star => "*".into(),
            Token::Slash => "/".into(),
            Token::Percent => "%".into(),
            Token::Alternative => "//".into(),
            Token::Assign => "=".into(),
            Token::Eof => "end of input".into(),
        }
    }
}

/// A lexer for the expression language.
pub struct Lexer<'a> {
    chars: Vec<char>,
    pos: usize,
    _marker: std::marker::PhantomData<&'a str>,
}

impl<'a> Lexer<'a> {
    /// Creates a new lexer over the given source.
    pub fn new(src: &'a str) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
            _marker: std::marker::PhantomData,
        }
    }

    /// Tokenizes the whole input.
    pub fn tokenize(mut self) -> Result<Vec<Token>, ExpressionError> {
        let mut tokens = Vec::new();
        loop {
            let batch = self.next_tokens()?;
            let is_eof = matches!(batch.first(), Some(Token::Eof));
            tokens.extend(batch);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    /// Reads the next batch of tokens (a string can produce several).
    fn next_tokens(&mut self) -> Result<Vec<Token>, ExpressionError> {
        // Skip whitespace and comments.
        loop {
            while let Some(c) = self.peek() {
                if c.is_whitespace() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if self.peek() == Some('#') {
                while let Some(ch) = self.peek() {
                    if ch == '\n' {
                        break;
                    }
                    self.pos += 1;
                }
                continue;
            }
            break;
        }

        let Some(c) = self.peek() else {
            return Ok(vec![Token::Eof]);
        };

        let pos = self.pos;

        // Multi-character operators.
        if c == '.' && self.peek2() == Some('.') {
            self.pos += 2;
            return Ok(vec![Token::RecursiveDot]);
        }
        if c == '=' && self.peek2() == Some('=') {
            self.pos += 2;
            return Ok(vec![Token::EqEq]);
        }
        if c == '!' && self.peek2() == Some('=') {
            self.pos += 2;
            return Ok(vec![Token::Ne]);
        }
        if c == '<' && self.peek2() == Some('=') {
            self.pos += 2;
            return Ok(vec![Token::Le]);
        }
        if c == '>' && self.peek2() == Some('=') {
            self.pos += 2;
            return Ok(vec![Token::Ge]);
        }
        if c == '/' && self.peek2() == Some('/') {
            self.pos += 2;
            return Ok(vec![Token::Alternative]);
        }

        let single = match c {
            '.' => {
                self.pos += 1;
                Token::Dot
            }
            '[' => {
                self.pos += 1;
                Token::LBracket
            }
            ']' => {
                self.pos += 1;
                Token::RBracket
            }
            '{' => {
                self.pos += 1;
                Token::LBrace
            }
            '}' => {
                self.pos += 1;
                Token::RBrace
            }
            '(' => {
                self.pos += 1;
                Token::LParen
            }
            ')' => {
                self.pos += 1;
                Token::RParen
            }
            '|' => {
                self.pos += 1;
                Token::Pipe
            }
            ',' => {
                self.pos += 1;
                Token::Comma
            }
            ':' => {
                self.pos += 1;
                Token::Colon
            }
            ';' => {
                self.pos += 1;
                Token::Semicolon
            }
            '+' => {
                self.pos += 1;
                Token::Plus
            }
            '-' => {
                self.pos += 1;
                Token::Minus
            }
            '<' => {
                self.pos += 1;
                Token::Lt
            }
            '>' => {
                self.pos += 1;
                Token::Gt
            }
            '*' => {
                self.pos += 1;
                Token::Star
            }
            '/' => {
                self.pos += 1;
                Token::Slash
            }
            '%' => {
                self.pos += 1;
                Token::Percent
            }
            '=' => {
                self.pos += 1;
                Token::Assign
            }
            '$' => {
                self.pos += 1;
                let name = self.read_ident();
                Token::Variable(name)
            }
            '"' => return self.read_string(),
            '0'..='9' => {
                let n = self.read_number()?;
                Token::Number(n)
            }
            _ if c.is_alphabetic() || c == '_' => {
                let ident = self.read_ident();
                Token::Ident(ident)
            }
            _ => {
                return Err(ExpressionError::Lex {
                    pos,
                    message: format!("unexpected character `{c}`"),
                })
            }
        };
        Ok(vec![single])
    }

    fn read_ident(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.chars[start..self.pos].iter().collect()
    }

    fn read_number(&mut self) -> Result<f64, ExpressionError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.peek() == Some('.') {
            self.pos += 1;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        // Handle exponent.
        if matches!(self.peek(), Some('e' | 'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.pos += 1;
            }
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse::<f64>().map_err(|_| ExpressionError::Lex {
            pos: start,
            message: format!("invalid number `{text}`"),
        })
    }

    /// Reads a double-quoted string, producing literal `Str` and `Interp` tokens.
    fn read_string(&mut self) -> Result<Vec<Token>, ExpressionError> {
        // Consume opening quote.
        self.pos += 1;
        let mut tokens = Vec::new();
        let mut literal = String::new();

        let flush = |tokens: &mut Vec<Token>, literal: &mut String| {
            if !literal.is_empty() {
                tokens.push(Token::Str(std::mem::take(literal)));
            }
        };

        loop {
            let Some(c) = self.bump() else {
                return Err(ExpressionError::Lex {
                    pos: self.pos,
                    message: "unterminated string".into(),
                });
            };
            match c {
                '"' => break,
                '\\' => {
                    let Some(esc) = self.bump() else {
                        return Err(ExpressionError::Lex {
                            pos: self.pos,
                            message: "unterminated escape".into(),
                        });
                    };
                    match esc {
                        '(' => {
                            // Interpolation: read a balanced `\( ... )` expression.
                            flush(&mut tokens, &mut literal);
                            let source = self.read_interp_source()?;
                            tokens.push(Token::Interp(source));
                        }
                        '"' => literal.push('"'),
                        '\\' => literal.push('\\'),
                        'n' => literal.push('\n'),
                        't' => literal.push('\t'),
                        'r' => literal.push('\r'),
                        'f' => literal.push('\u{000c}'),
                        'b' => literal.push('\u{0008}'),
                        '/' => literal.push('/'),
                        'u' => {
                            let hex: String = (0..4).filter_map(|_| self.bump()).collect();
                            let code = u32::from_str_radix(&hex, 16).map_err(|_| {
                                ExpressionError::Lex {
                                    pos: self.pos,
                                    message: "invalid unicode escape".into(),
                                }
                            })?;
                            if let Some(ch) = char::from_u32(code) {
                                literal.push(ch);
                            }
                        }
                        other => {
                            literal.push('\\');
                            literal.push(other);
                        }
                    }
                }
                other => literal.push(other),
            }
        }
        flush(&mut tokens, &mut literal);
        if tokens.is_empty() {
            tokens.push(Token::Str(String::new()));
        }
        Ok(tokens)
    }

    /// Reads the source of a `\( ... )` interpolation, balancing nested parens.
    fn read_interp_source(&mut self) -> Result<String, ExpressionError> {
        let mut depth = 1;
        let start = self.pos;
        loop {
            let Some(c) = self.bump() else {
                return Err(ExpressionError::Lex {
                    pos: self.pos,
                    message: "unterminated interpolation".into(),
                });
            };
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        Ok(self.chars[start..self.pos - 1].iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Token> {
        Lexer::new(src).tokenize().unwrap()
    }

    #[test]
    fn basic_tokens() {
        let t = toks(".foo + 1");
        assert!(matches!(t[0], Token::Dot));
        assert!(matches!(t[1], Token::Ident(ref s) if s == "foo"));
        assert!(matches!(t[2], Token::Plus));
        assert!(matches!(t[3], Token::Number(1.0)));
        assert!(matches!(t[4], Token::Eof));
    }

    #[test]
    fn variables_numbers_and_strings() {
        let t = toks("$x \"hi\" 3.5");
        assert!(matches!(t[0], Token::Variable(ref s) if s == "x"));
        assert!(matches!(t[1], Token::Str(ref s) if s == "hi"));
        assert!(matches!(t[2], Token::Number(3.5)));
    }

    #[test]
    fn multi_char_operators() {
        let t = toks("== != <= >= //");
        assert!(matches!(t[0], Token::EqEq));
        assert!(matches!(t[1], Token::Ne));
        assert!(matches!(t[2], Token::Le));
        assert!(matches!(t[3], Token::Ge));
        assert!(matches!(t[4], Token::Alternative));
    }

    #[test]
    fn string_with_interpolation() {
        let t = toks(r#""a \(.b) c""#);
        assert!(matches!(t[0], Token::Str(ref s) if s == "a "));
        assert!(matches!(t[1], Token::Interp(ref s) if s == ".b"));
        assert!(matches!(t[2], Token::Str(ref s) if s == " c"));
    }

    #[test]
    fn comments_and_whitespace() {
        let t = toks("1 # comment\n 2");
        assert!(matches!(t[0], Token::Number(1.0)));
        assert!(matches!(t[1], Token::Number(2.0)));
    }

    #[test]
    fn lex_errors() {
        assert!(Lexer::new("\"unterminated").tokenize().is_err());
        assert!(Lexer::new("`").tokenize().is_err());
    }

    #[test]
    fn structural_tokens() {
        let t = toks("{ } ( ) | , : ; ..");
        assert!(matches!(t[0], Token::LBrace));
        assert!(matches!(t[1], Token::RBrace));
        assert!(matches!(t[2], Token::LParen));
        assert!(matches!(t[3], Token::RParen));
        assert!(matches!(t[4], Token::Pipe));
        assert!(matches!(t[5], Token::Comma));
        assert!(matches!(t[6], Token::Colon));
        assert!(matches!(t[7], Token::Semicolon));
        assert!(matches!(t[8], Token::RecursiveDot));
    }

    #[test]
    fn describe_is_informative() {
        assert_eq!(Token::Pipe.describe(), "|");
        assert_eq!(Token::Eof.describe(), "end of input");
        assert!(Token::Ident("foo".into()).describe() == "foo");
    }

    #[test]
    fn number_exponent_and_negative() {
        let t = toks("1e3 -2.5");
        assert!(matches!(t[0], Token::Number(n) if n == 1000.0));
        assert!(matches!(t[1], Token::Minus));
        assert!(matches!(t[2], Token::Number(n) if n == 2.5));
    }

    #[test]
    fn quoted_string_token() {
        let t = toks("\"hello\"");
        assert!(matches!(t[0], Token::Str(ref s) if s == "hello"));
    }

    #[test]
    fn all_describe_arms() {
        for t in [
            Token::Dot,
            Token::RecursiveDot,
            Token::LBracket,
            Token::RBracket,
            Token::LBrace,
            Token::RBrace,
            Token::LParen,
            Token::RParen,
            Token::Comma,
            Token::Colon,
            Token::Semicolon,
            Token::EqEq,
            Token::Ne,
            Token::Lt,
            Token::Le,
            Token::Gt,
            Token::Ge,
            Token::Plus,
            Token::Minus,
            Token::Star,
            Token::Slash,
            Token::Percent,
            Token::Alternative,
            Token::Assign,
            Token::Eof,
            Token::Variable("v".into()),
            Token::Number(1.0),
            Token::Str("s".into()),
            Token::Interp("i".into()),
            Token::Ident("id".into()),
        ] {
            assert!(!t.describe().is_empty());
        }
    }

    #[test]
    fn string_escape_arms() {
        // tab, carriage return, form feed, backspace, forward slash, unicode.
        let t = toks(r#""a	b""#);
        if let Token::Str(ref s) = t[0] {
            assert!(s.contains('\t'));
        } else {
            panic!();
        }
        let t = toks(
            r#""a
b""#,
        );
        if let Token::Str(ref s) = t[0] {
            assert!(s.contains('\n'));
        } else {
            panic!();
        }
        let t = toks(r#""a\/b""#);
        if let Token::Str(ref s) = t[0] {
            assert!(s.contains('/'));
        } else {
            panic!();
        }
        let t = toks(r#""\u00e9""#);
        if let Token::Str(ref s) = t[0] {
            assert_eq!(s, "\u{e9}");
        } else {
            panic!();
        }
        // invalid unicode escape errors
        assert!(Lexer::new(r#""\uZZZZ""#).tokenize().is_err());
    }
}

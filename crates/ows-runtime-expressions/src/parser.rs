//! Recursive-descent parser for the sandboxed expression language.

use serde_json::Value;

use super::ast::{BinOp, Expr, InterpPart, ObjKey, UnaryOp};
use super::error::ExpressionError;
use super::lexer::{Lexer, Token};

/// Parses a complete expression from source text.
pub fn parse(source: &str) -> Result<Expr, ExpressionError> {
    let tokens = Lexer::new(source).tokenize()?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_expr()?;
    if !matches!(parser.peek(), Token::Eof) {
        return Err(ExpressionError::parse(
            parser.pos,
            format!("unexpected `{}`", parser.peek().describe()),
        ));
    }
    Ok(expr)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek2(&self) -> &Token {
        &self.tokens[(self.pos + 1).min(self.tokens.len() - 1)]
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, tok: &Token, what: &str) -> Result<(), ExpressionError> {
        if self.peek() == tok {
            self.advance();
            Ok(())
        } else {
            Err(ExpressionError::parse(
                self.pos,
                format!("expected {what}, found `{}`", self.peek().describe()),
            ))
        }
    }

    // expr := comma
    fn parse_expr(&mut self) -> Result<Expr, ExpressionError> {
        let mut parts = vec![self.parse_pipe()?];
        while matches!(self.peek(), Token::Comma) {
            self.advance();
            parts.push(self.parse_pipe()?);
        }
        if parts.len() == 1 {
            Ok(parts.pop().unwrap())
        } else {
            Ok(Expr::Stream(parts))
        }
    }

    // pipe := or ("|" or)*
    fn parse_pipe(&mut self) -> Result<Expr, ExpressionError> {
        let mut left = self.parse_or()?;
        while matches!(self.peek(), Token::Pipe) {
            self.advance();
            let right = self.parse_or()?;
            left = Expr::Pipe(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // or := and ("or" and)*
    fn parse_or(&mut self) -> Result<Expr, ExpressionError> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Token::Ident(i) if i == "or") {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::Binary(BinOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // and := compare ("and" compare)*
    fn parse_and(&mut self) -> Result<Expr, ExpressionError> {
        let mut left = self.parse_compare()?;
        while matches!(self.peek(), Token::Ident(i) if i == "and") {
            self.advance();
            let right = self.parse_compare()?;
            left = Expr::Binary(BinOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // compare := additive (comp additive)*
    fn parse_compare(&mut self) -> Result<Expr, ExpressionError> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Token::EqEq => BinOp::Eq,
                Token::Ne => BinOp::Ne,
                Token::Lt => BinOp::Lt,
                Token::Le => BinOp::Le,
                Token::Gt => BinOp::Gt,
                Token::Ge => BinOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.parse_additive()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // additive := mult (("+" | "-" | "//") mult)*
    fn parse_additive(&mut self) -> Result<Expr, ExpressionError> {
        let mut left = self.parse_mult()?;
        loop {
            let op = match self.peek() {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                Token::Alternative => BinOp::Alternative,
                _ => break,
            };
            self.advance();
            let right = self.parse_mult()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // mult := unary (("*" | "/" | "%") unary)*
    fn parse_mult(&mut self) -> Result<Expr, ExpressionError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    // unary := ("not" | "-") unary | postfix
    fn parse_unary(&mut self) -> Result<Expr, ExpressionError> {
        if matches!(self.peek(), Token::Ident(i) if i == "not") {
            self.advance();
            let inner = self.parse_unary()?;
            return Ok(Expr::Unary(UnaryOp::Not, Box::new(inner)));
        }
        if matches!(self.peek(), Token::Minus) {
            // Check it is a unary minus (not part of a number handled by lexer).
            self.advance();
            let inner = self.parse_unary()?;
            return Ok(Expr::Unary(UnaryOp::Negate, Box::new(inner)));
        }
        self.parse_postfix()
    }

    // postfix := base (("." ident) | ("[" "]") | ("[" expr "]"))*
    fn parse_postfix(&mut self) -> Result<Expr, ExpressionError> {
        let mut base = self.parse_base()?;
        loop {
            match self.peek() {
                Token::Dot => {
                    // `.foo` field access.
                    let next = self.peek2().clone();
                    match next {
                        Token::Ident(name) | Token::Str(name) => {
                            self.advance(); // consume dot
                            self.advance(); // consume name
                            base = Expr::Field(Box::new(base), name);
                        }
                        _ => break,
                    }
                }
                Token::LBracket => {
                    if matches!(self.peek2(), Token::RBracket) {
                        self.advance();
                        self.advance();
                        base = Expr::Iterate(Box::new(base));
                    } else {
                        self.advance();
                        let index = self.parse_expr()?;
                        self.expect(&Token::RBracket, "`]`")?;
                        base = Expr::Index(Box::new(base), Box::new(index));
                    }
                }
                _ => break,
            }
        }
        Ok(base)
    }

    fn parse_base(&mut self) -> Result<Expr, ExpressionError> {
        match self.peek() {
            Token::Dot => {
                self.advance();
                let mut base = Expr::Identity;
                // Parse a field-access chain like `.foo.bar[0].baz`.
                loop {
                    match self.peek() {
                        Token::Ident(name) | Token::Str(name) => {
                            let name = name.clone();
                            self.advance();
                            base = Expr::Field(Box::new(base), name);
                        }
                        Token::LBracket => {
                            if matches!(self.peek2(), Token::RBracket) {
                                self.advance();
                                self.advance();
                                base = Expr::Iterate(Box::new(base));
                            } else {
                                self.advance();
                                let index = self.parse_expr()?;
                                self.expect(&Token::RBracket, "`]`")?;
                                base = Expr::Index(Box::new(base), Box::new(index));
                            }
                        }
                        _ => break,
                    }
                }
                Ok(base)
            }
            Token::RecursiveDot => {
                self.advance();
                // Treat `..` as identity (limited recursive descent support).
                Ok(Expr::Identity)
            }
            Token::Variable(name) => {
                let name = name.clone();
                self.advance();
                Ok(Expr::Variable(name))
            }
            Token::Number(n) => {
                let n = *n;
                self.advance();
                Ok(Expr::Literal(json_num(n)))
            }
            Token::Str(_) | Token::Interp(_) => self.parse_string_parts(),
            Token::LBracket => self.parse_array(),
            Token::LBrace => self.parse_object(),
            Token::LParen => {
                self.advance();
                let e = self.parse_pipe()?;
                self.expect(&Token::RParen, "`)`")?;
                Ok(e)
            }
            Token::Ident(ident) => self.parse_ident_primary(ident.clone()),
            Token::Eof => Err(ExpressionError::parse(
                self.pos,
                "unexpected end of expression",
            )),
            other => Err(ExpressionError::parse(
                self.pos,
                format!("unexpected `{}`", other.describe()),
            )),
        }
    }

    fn parse_ident_primary(&mut self, ident: String) -> Result<Expr, ExpressionError> {
        match ident.as_str() {
            "true" => {
                self.advance();
                Ok(Expr::Literal(Value::Bool(true)))
            }
            "false" => {
                self.advance();
                Ok(Expr::Literal(Value::Bool(false)))
            }
            "null" => {
                self.advance();
                Ok(Expr::Literal(Value::Null))
            }
            "if" => self.parse_if(),
            _ => {
                // Function call: ident "(" args ")".
                if matches!(self.peek2(), Token::LParen) {
                    self.advance();
                    self.advance();
                    let args = self.parse_args()?;
                    self.expect(&Token::RParen, "`)`")?;
                    Ok(Expr::Call(ident, args))
                } else {
                    Err(ExpressionError::parse(
                        self.pos,
                        format!("unknown identifier `{ident}`"),
                    ))
                }
            }
        }
    }

    fn parse_if(&mut self) -> Result<Expr, ExpressionError> {
        self.advance(); // `if`
        let cond = self.parse_expr()?;
        self.expect_keyword("then")?;
        let then_branch = self.parse_expr()?;
        // Optional `elif`.
        if matches!(self.peek(), Token::Ident(i) if i == "elif") {
            self.advance();
            let elif_cond = self.parse_expr()?;
            self.expect_keyword("then")?;
            let elif_body = self.parse_expr()?;
            let else_branch = self.parse_else_tail()?;
            let nested = Expr::If(
                Box::new(elif_cond),
                Box::new(elif_body),
                Box::new(else_branch),
            );
            self.expect_keyword("end")?;
            return Ok(Expr::If(
                Box::new(cond),
                Box::new(then_branch),
                Box::new(nested),
            ));
        }
        let else_branch = self.parse_else_tail()?;
        self.expect_keyword("end")?;
        Ok(Expr::If(
            Box::new(cond),
            Box::new(then_branch),
            Box::new(else_branch),
        ))
    }

    fn parse_else_tail(&mut self) -> Result<Expr, ExpressionError> {
        if matches!(self.peek(), Token::Ident(i) if i == "else") {
            self.advance();
            self.parse_expr()
        } else if matches!(self.peek(), Token::Ident(i) if i == "elif") {
            // handled by caller for elif; fallback:
            self.parse_expr()
        } else {
            // Implicit else is empty.
            Ok(Expr::Literal(Value::Null))
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), ExpressionError> {
        if matches!(self.peek(), Token::Ident(i) if i == kw) {
            self.advance();
            Ok(())
        } else {
            Err(ExpressionError::parse(
                self.pos,
                format!("expected `{kw}`, found `{}`", self.peek().describe()),
            ))
        }
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, ExpressionError> {
        let mut args = Vec::new();
        if matches!(self.peek(), Token::RParen) {
            return Ok(args);
        }
        loop {
            args.push(self.parse_pipe()?);
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(args)
    }

    fn parse_array(&mut self) -> Result<Expr, ExpressionError> {
        self.expect(&Token::LBracket, "`[`")?;
        let mut elems = Vec::new();
        if matches!(self.peek(), Token::RBracket) {
            self.advance();
            return Ok(Expr::Array(elems));
        }
        loop {
            let e = self.parse_pipe()?;
            // A stream inside array is flattened into elements.
            match e {
                Expr::Stream(parts) => elems.extend(parts),
                other => elems.push(other),
            }
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(&Token::RBracket, "`]`")?;
        Ok(Expr::Array(elems))
    }

    fn parse_object(&mut self) -> Result<Expr, ExpressionError> {
        self.expect(&Token::LBrace, "`{`")?;
        let mut entries: Vec<(ObjKey, Expr)> = Vec::new();
        if matches!(self.peek(), Token::RBrace) {
            self.advance();
            return Ok(Expr::Object(entries));
        }
        loop {
            let key = self.parse_object_key()?;
            self.expect(&Token::Colon, "`:`")?;
            let value = self.parse_pipe()?;
            entries.push((key, value));
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(&Token::RBrace, "`}`")?;
        Ok(Expr::Object(entries))
    }

    fn parse_object_key(&mut self) -> Result<ObjKey, ExpressionError> {
        match self.peek() {
            Token::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(ObjKey::Ident(name))
            }
            Token::Str(s) => {
                let s = s.clone();
                self.advance();
                Ok(ObjKey::Str(s))
            }
            Token::Variable(name) => {
                let name = name.clone();
                self.advance();
                Ok(ObjKey::Ident(name))
            }
            Token::LParen => {
                self.advance();
                let e = self.parse_pipe()?;
                self.expect(&Token::RParen, "`)`")?;
                Ok(ObjKey::Computed(Box::new(e)))
            }
            other => Err(ExpressionError::parse(
                self.pos,
                format!("expected object key, found `{}`", other.describe()),
            )),
        }
    }

    fn parse_string_parts(&mut self) -> Result<Expr, ExpressionError> {
        let mut parts = Vec::new();
        loop {
            match self.peek() {
                Token::Str(s) => {
                    let s = s.clone();
                    self.advance();
                    parts.push(InterpPart::Literal(s));
                }
                Token::Interp(src) => {
                    let src = src.clone();
                    self.advance();
                    let inner = parse(&src)?;
                    parts.push(InterpPart::Expr(Box::new(inner)));
                }
                _ => break,
            }
        }
        Ok(Expr::Str(parts))
    }
}

fn json_num(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        Value::Number(serde_json::Number::from(n as i64))
    } else {
        serde_json::Number::from_f64(n)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

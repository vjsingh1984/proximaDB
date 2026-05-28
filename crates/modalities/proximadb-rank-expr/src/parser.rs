//! Hand-rolled recursive-descent parser for the rank expression DSL.
//!
//! Grammar lives in `lib.rs`. Per-expression caps (max depth 256, max
//! nodes 1024) are enforced at parse time so a malicious profile can't
//! blow the stack.

use crate::ast::{Ast, BinOp};
use proximadb_rank_core::RankError;

const MAX_DEPTH: usize = 256;
const MAX_NODES: usize = 1024;

pub fn parse(input: &str) -> Result<Ast, RankError> {
    let mut p = Parser::new(input);
    p.skip_ws();
    if p.is_eof() {
        return Err(RankError::ExpressionParse("empty expression".into()));
    }
    let ast = p.parse_expr(0)?;
    p.skip_ws();
    if !p.is_eof() {
        return Err(RankError::ExpressionParse(format!(
            "unexpected trailing input at position {}: {:?}",
            p.pos,
            p.rest_preview()
        )));
    }
    if ast.depth() > MAX_DEPTH {
        return Err(RankError::DependencyTooDeep { max: MAX_DEPTH });
    }
    if ast.node_count() > MAX_NODES {
        return Err(RankError::ExpressionParse(format!(
            "expression has {} nodes, exceeds cap of {}",
            ast.node_count(),
            MAX_NODES
        )));
    }
    Ok(ast)
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            src: input.as_bytes(),
            pos: 0,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn try_consume(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn rest_preview(&self) -> String {
        let n = (self.src.len() - self.pos).min(16);
        String::from_utf8_lossy(&self.src[self.pos..self.pos + n]).to_string()
    }

    // ---------------- expression precedence climbing ----------------
    //
    // expr      -> add
    // add       -> mul (('+'|'-') mul)*    (left-assoc)
    // mul       -> unary (('*'|'/') unary)*(left-assoc)
    // unary     -> '-' unary | pow
    // pow       -> atom ('^' unary)?       (right-assoc — unary recurses)
    // atom      -> number | string | call | '(' expr ')'

    fn parse_expr(&mut self, depth: usize) -> Result<Ast, RankError> {
        if depth > MAX_DEPTH {
            return Err(RankError::DependencyTooDeep { max: MAX_DEPTH });
        }
        self.parse_add(depth)
    }

    fn parse_add(&mut self, depth: usize) -> Result<Ast, RankError> {
        let mut lhs = self.parse_mul(depth + 1)?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.bump();
                    let rhs = self.parse_mul(depth + 1)?;
                    lhs = Ast::Bin(BinOp::Add, Box::new(lhs), Box::new(rhs));
                }
                Some(b'-') => {
                    self.bump();
                    let rhs = self.parse_mul(depth + 1)?;
                    lhs = Ast::Bin(BinOp::Sub, Box::new(lhs), Box::new(rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_mul(&mut self, depth: usize) -> Result<Ast, RankError> {
        let mut lhs = self.parse_unary(depth + 1)?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.bump();
                    let rhs = self.parse_unary(depth + 1)?;
                    lhs = Ast::Bin(BinOp::Mul, Box::new(lhs), Box::new(rhs));
                }
                Some(b'/') => {
                    self.bump();
                    let rhs = self.parse_unary(depth + 1)?;
                    lhs = Ast::Bin(BinOp::Div, Box::new(lhs), Box::new(rhs));
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn parse_unary(&mut self, depth: usize) -> Result<Ast, RankError> {
        self.skip_ws();
        if self.peek() == Some(b'-') {
            self.bump();
            let inner = self.parse_unary(depth + 1)?;
            return Ok(Ast::Neg(Box::new(inner)));
        }
        self.parse_pow(depth + 1)
    }

    fn parse_pow(&mut self, depth: usize) -> Result<Ast, RankError> {
        let base = self.parse_atom(depth + 1)?;
        self.skip_ws();
        if self.peek() == Some(b'^') {
            self.bump();
            // right-associative: ^ recurses through unary
            let exp = self.parse_unary(depth + 1)?;
            return Ok(Ast::Bin(BinOp::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn parse_atom(&mut self, depth: usize) -> Result<Ast, RankError> {
        self.skip_ws();
        match self.peek() {
            None => Err(RankError::ExpressionParse("unexpected end of input".into())),
            Some(b'(') => {
                self.bump();
                let inner = self.parse_expr(depth + 1)?;
                self.skip_ws();
                if !self.try_consume(b')') {
                    return Err(RankError::ExpressionParse(format!(
                        "expected ')' at position {}",
                        self.pos
                    )));
                }
                Ok(inner)
            }
            Some(b'"') | Some(b'\'') => self.parse_string(),
            Some(b) if b.is_ascii_digit() || b == b'.' => self.parse_number(),
            Some(b) if Self::is_ident_start(b) => self.parse_call_or_ident(depth + 1),
            Some(b) => Err(RankError::ExpressionParse(format!(
                "unexpected character {:?} at position {}",
                b as char, self.pos
            ))),
        }
    }

    fn parse_string(&mut self) -> Result<Ast, RankError> {
        let quote = self.bump().expect("caller checked");
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == quote {
                let s = std::str::from_utf8(&self.src[start..self.pos])
                    .map_err(|e| {
                        RankError::ExpressionParse(format!("invalid utf-8 in string: {e}"))
                    })?
                    .to_string();
                self.bump();
                return Ok(Ast::Str(s));
            }
            self.bump();
        }
        Err(RankError::ExpressionParse(format!(
            "unterminated string starting at position {start}"
        )))
    }

    fn parse_number(&mut self) -> Result<Ast, RankError> {
        let start = self.pos;
        // integer part
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                self.bump();
            } else {
                break;
            }
        }
        // fractional
        if self.peek() == Some(b'.') && matches!(self.peek_at(1), Some(b) if b.is_ascii_digit()) {
            self.bump();
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        // exponent
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.bump();
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.bump();
            }
            let exp_start = self.pos;
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    self.bump();
                } else {
                    break;
                }
            }
            if self.pos == exp_start {
                return Err(RankError::ExpressionParse("exponent has no digits".into()));
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|e| RankError::ExpressionParse(format!("invalid utf-8 in number: {e}")))?;
        let v: f64 = text
            .parse()
            .map_err(|e| RankError::ExpressionParse(format!("bad number {text:?}: {e}")))?;
        Ok(Ast::Num(v))
    }

    fn parse_call_or_ident(&mut self, depth: usize) -> Result<Ast, RankError> {
        let name = self.parse_ident()?;
        self.skip_ws();
        if self.peek() != Some(b'(') {
            // Bare identifier — interpret as a zero-arg call. The lowering
            // pass decides whether it's a feature (e.g. `nativeRank`)
            // or an unknown reference.
            return Ok(Ast::Call(name, Vec::new()));
        }
        self.bump(); // '('
        let mut args = Vec::new();
        self.skip_ws();
        if self.peek() != Some(b')') {
            loop {
                let a = self.parse_expr(depth + 1)?;
                args.push(a);
                self.skip_ws();
                match self.peek() {
                    Some(b',') => {
                        self.bump();
                        self.skip_ws();
                    }
                    Some(b')') => break,
                    _ => {
                        return Err(RankError::ExpressionParse(format!(
                            "expected ',' or ')' in argument list at position {}",
                            self.pos
                        )));
                    }
                }
            }
        }
        if !self.try_consume(b')') {
            return Err(RankError::ExpressionParse(format!(
                "expected ')' to close argument list at position {}",
                self.pos
            )));
        }
        // Special-case `if(cond, then, else)` — emit dedicated If node.
        if name == "if" {
            if args.len() != 3 {
                return Err(RankError::ExpressionParse(format!(
                    "if(...) takes exactly 3 arguments, got {}",
                    args.len()
                )));
            }
            let mut it = args.into_iter();
            let c = it.next().unwrap();
            let t = it.next().unwrap();
            let e = it.next().unwrap();
            return Ok(Ast::If(Box::new(c), Box::new(t), Box::new(e)));
        }
        Ok(Ast::Call(name, args))
    }

    fn parse_ident(&mut self) -> Result<String, RankError> {
        let start = self.pos;
        match self.peek() {
            Some(b) if Self::is_ident_start(b) => self.bump(),
            _ => {
                return Err(RankError::ExpressionParse(format!(
                    "expected identifier at position {}",
                    self.pos
                )));
            }
        };
        while let Some(b) = self.peek() {
            if Self::is_ident_cont(b) {
                self.bump();
            } else {
                break;
            }
        }
        let s = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|e| RankError::ExpressionParse(format!("invalid utf-8 in identifier: {e}")))?
            .to_string();
        Ok(s)
    }

    fn is_ident_start(b: u8) -> bool {
        b.is_ascii_alphabetic() || b == b'_'
    }
    fn is_ident_cont(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Ast {
        parse(s).unwrap_or_else(|e| panic!("parse failed for {s:?}: {e:?}"))
    }
    fn p_err(s: &str) -> RankError {
        parse(s).unwrap_err()
    }

    #[test]
    fn parses_number_literal() {
        match p("42") {
            Ast::Num(v) => assert_eq!(v, 42.0),
            other => panic!("expected Num, got {other:?}"),
        }
    }

    #[test]
    fn parses_floating_point() {
        match p("3.5") {
            Ast::Num(v) => assert!((v - 3.5).abs() < 1e-9),
            other => panic!("expected Num, got {other:?}"),
        }
    }

    #[test]
    fn parses_scientific_notation() {
        match p("1.5e3") {
            Ast::Num(v) => assert!((v - 1500.0).abs() < 1e-6),
            other => panic!("expected Num, got {other:?}"),
        }
    }

    #[test]
    fn parses_addition_left_associative() {
        // "1 - 2 - 3" must parse as (1 - 2) - 3 = -4, not 1 - (2 - 3) = 2
        let a = p("1 - 2 - 3");
        match a {
            Ast::Bin(BinOp::Sub, lhs, rhs) => {
                // rhs must be a single number, not a Bin(Sub,…)
                assert_eq!(*rhs, Ast::Num(3.0));
                match *lhs {
                    Ast::Bin(BinOp::Sub, _, _) => {} // ok
                    other => panic!("expected nested Sub on lhs, got {other:?}"),
                }
            }
            other => panic!("expected Bin(Sub,…), got {other:?}"),
        }
    }

    #[test]
    fn mul_binds_tighter_than_add() {
        // "1 + 2 * 3" → 1 + (2*3)
        let a = p("1 + 2 * 3");
        match a {
            Ast::Bin(BinOp::Add, lhs, rhs) => {
                assert_eq!(*lhs, Ast::Num(1.0));
                match *rhs {
                    Ast::Bin(BinOp::Mul, _, _) => {}
                    other => panic!("expected Mul on rhs, got {other:?}"),
                }
            }
            other => panic!("expected Add at root, got {other:?}"),
        }
    }

    #[test]
    fn parens_override_precedence() {
        // "(1 + 2) * 3" → (1+2) * 3
        let a = p("(1 + 2) * 3");
        match a {
            Ast::Bin(BinOp::Mul, lhs, rhs) => {
                assert_eq!(*rhs, Ast::Num(3.0));
                match *lhs {
                    Ast::Bin(BinOp::Add, _, _) => {}
                    other => panic!("expected Add inside parens, got {other:?}"),
                }
            }
            other => panic!("expected Mul at root, got {other:?}"),
        }
    }

    #[test]
    fn pow_is_right_associative() {
        // "2 ^ 3 ^ 2" → 2 ^ (3^2) = 512, not (2^3)^2 = 64
        let a = p("2 ^ 3 ^ 2");
        match a {
            Ast::Bin(BinOp::Pow, lhs, rhs) => {
                assert_eq!(*lhs, Ast::Num(2.0));
                match *rhs {
                    Ast::Bin(BinOp::Pow, _, _) => {}
                    other => panic!("expected Pow on rhs, got {other:?}"),
                }
            }
            other => panic!("expected Pow at root, got {other:?}"),
        }
    }

    #[test]
    fn unary_minus_wraps_atom() {
        let a = p("-5");
        match a {
            Ast::Neg(inner) => assert_eq!(*inner, Ast::Num(5.0)),
            other => panic!("expected Neg, got {other:?}"),
        }
    }

    #[test]
    fn double_unary_minus() {
        let a = p("--5");
        match a {
            Ast::Neg(inner) => match *inner {
                Ast::Neg(_) => {}
                other => panic!("expected nested Neg, got {other:?}"),
            },
            other => panic!("expected Neg, got {other:?}"),
        }
    }

    #[test]
    fn parses_function_call_with_args() {
        let a = p("max(1, 2)");
        match a {
            Ast::Call(name, args) => {
                assert_eq!(name, "max");
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn parses_feature_with_string_arg() {
        let a = p("bm25(\"title\")");
        match a {
            Ast::Call(name, args) => {
                assert_eq!(name, "bm25");
                assert_eq!(args.len(), 1);
                match &args[0] {
                    Ast::Str(s) => assert_eq!(s, "title"),
                    other => panic!("expected Str arg, got {other:?}"),
                }
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn parses_single_quoted_string() {
        let a = p("bm25('title')");
        match a {
            Ast::Call(_, args) => match &args[0] {
                Ast::Str(s) => assert_eq!(s, "title"),
                other => panic!("expected Str, got {other:?}"),
            },
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn parses_bare_identifier_as_zero_arg_call() {
        let a = p("nativeRank");
        match a {
            Ast::Call(name, args) => {
                assert_eq!(name, "nativeRank");
                assert!(args.is_empty());
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn parses_if_as_dedicated_node() {
        let a = p("if(1, 2, 3)");
        match a {
            Ast::If(c, t, e) => {
                assert_eq!(*c, Ast::Num(1.0));
                assert_eq!(*t, Ast::Num(2.0));
                assert_eq!(*e, Ast::Num(3.0));
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn realistic_first_phase_expression() {
        // The example from the spec §4.5.1.
        let a = p("closeness(\"embedding\") * 0.6 + bm25(\"title\") * 0.4");
        match a {
            Ast::Bin(BinOp::Add, _, _) => {} // root is +
            other => panic!("expected Add at root, got {other:?}"),
        }
    }

    // ---------------- error paths ----------------

    #[test]
    fn empty_input_errors() {
        let e = p_err("");
        assert!(matches!(e, RankError::ExpressionParse(_)));
    }

    #[test]
    fn whitespace_only_errors() {
        let e = p_err("   ");
        assert!(matches!(e, RankError::ExpressionParse(_)));
    }

    #[test]
    fn unbalanced_paren_errors() {
        let e = p_err("(1 + 2");
        assert!(matches!(e, RankError::ExpressionParse(_)));
    }

    #[test]
    fn unterminated_string_errors() {
        let e = p_err("bm25(\"title)");
        assert!(matches!(e, RankError::ExpressionParse(_)));
    }

    #[test]
    fn trailing_garbage_errors() {
        let e = p_err("1 + 2 garbage");
        assert!(matches!(e, RankError::ExpressionParse(_)));
    }

    #[test]
    fn missing_comma_in_args_errors() {
        let e = p_err("max(1 2)");
        assert!(matches!(e, RankError::ExpressionParse(_)));
    }

    #[test]
    fn if_with_wrong_arity_errors() {
        let e = p_err("if(1, 2)");
        assert!(matches!(e, RankError::ExpressionParse(_)));
        let e = p_err("if(1, 2, 3, 4)");
        assert!(matches!(e, RankError::ExpressionParse(_)));
    }

    #[test]
    fn over_max_depth_rejected() {
        // Build an expression nested deeper than MAX_DEPTH parens
        let mut s = String::new();
        for _ in 0..(MAX_DEPTH + 10) {
            s.push('(');
        }
        s.push('1');
        for _ in 0..(MAX_DEPTH + 10) {
            s.push(')');
        }
        let e = parse(&s).unwrap_err();
        assert!(
            matches!(
                e,
                RankError::DependencyTooDeep { .. } | RankError::ExpressionParse(_)
            ),
            "got: {e:?}"
        );
    }
}

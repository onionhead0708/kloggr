//! Boolean combinations of quoted sub-patterns, e.g. `"error" and not("timeout")`.
//!
//! klogg (`src/regex/src/regularexpression.cpp`, `booleanevaluator.cpp`, GPLv3)
//! replaces each quoted sub-pattern with a variable and hands the expression to
//! exprtk. kloggr parses the same language itself:
//!
//! | operators | precedence (exprtk level) |
//! |---|---|
//! | `or`, `nor`, `xor`, `xnor`, `\|` (also `\|\|`) | lowest, left-associative |
//! | `and`, `nand`, `&` (also `&&`) | higher, left-associative |
//! | `not(…)`, `!` | unary, binds tightest |
//!
//! Keywords are case-insensitive. Inside a sub-pattern, `\"` stands for a
//! quote; other backslashes are left for the regex.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    And,
    Nand,
    Or,
    Nor,
    Xor,
    Xnor,
}

impl Op {
    fn apply(self, a: bool, b: bool) -> bool {
        match self {
            Op::And => a && b,
            Op::Nand => !(a && b),
            Op::Or => a || b,
            Op::Nor => !(a || b),
            Op::Xor => a != b,
            Op::Xnor => a == b,
        }
    }
}

/// A parsed expression over sub-patterns, numbered in order of appearance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Expr {
    Pattern(usize),
    Not(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Evaluates the expression given which sub-patterns match.
    pub(crate) fn eval(&self, matched: &impl Fn(usize) -> bool) -> bool {
        match self {
            Expr::Pattern(i) => matched(*i),
            Expr::Not(e) => !e.eval(matched),
            Expr::Binary(op, a, b) => {
                // `&` and `|` short-circuit in exprtk; with side-effect-free
                // operands every operator can.
                let a = a.eval(matched);
                match op {
                    Op::And if !a => false,
                    Op::Nand if !a => true,
                    Op::Or if a => true,
                    Op::Nor if a => false,
                    _ => op.apply(a, b.eval(matched)),
                }
            }
        }
    }
}

/// A syntax error, with the 1-based column (in chars) where it was found.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} at column {column}")]
pub struct BooleanError {
    pub message: String,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Pattern(usize),
    Op(Op),
    Not,
    Open,
    Close,
}

/// Parses `input` into an expression and its sub-patterns (unescaped).
pub(crate) fn parse(input: &str) -> Result<(Expr, Vec<String>), BooleanError> {
    let err = |message: &str, column: usize| BooleanError {
        message: message.to_owned(),
        column,
    };
    if !input.contains('"') {
        return Err(err("patterns must be enclosed in quotes", 1));
    }
    let (tokens, patterns) = tokenize(input)?;
    let end = input.chars().count() + 1;
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
        end,
    };
    let expr = parser.or_expr()?;
    if let Some((_, column)) = parser.peek() {
        return Err(err("expected an operator", column));
    }
    Ok((expr, patterns))
}

/// Tokens with their 1-based column, and the sub-patterns they refer to.
type Tokens = (Vec<(Token, usize)>, Vec<String>);

fn tokenize(input: &str) -> Result<Tokens, BooleanError> {
    let err = |message: &str, column: usize| BooleanError {
        message: message.to_owned(),
        column,
    };
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut patterns = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let column = i + 1;
        let c = chars[i];
        let token = match c {
            c if c.is_whitespace() => {
                i += 1;
                continue;
            }
            '"' => {
                // As in klogg, a quote preceded by a backslash is escaped.
                let close = (i + 1..chars.len())
                    .find(|&k| chars[k] == '"' && chars[k - 1] != '\\')
                    .ok_or_else(|| err("pattern has unmatched quotes", column))?;
                let text: String = chars[i + 1..close].iter().collect();
                patterns.push(text.replace("\\\"", "\""));
                i = close + 1;
                tokens.push((Token::Pattern(patterns.len() - 1), column));
                continue;
            }
            '(' => Token::Open,
            ')' => Token::Close,
            '!' => Token::Not,
            '&' | '|' => {
                if chars.get(i + 1) == Some(&c) {
                    i += 1;
                }
                Token::Op(if c == '&' { Op::And } else { Op::Or })
            }
            c if c.is_ascii_alphabetic() => {
                let len = chars[i..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                    .count();
                let word: String = chars[i..i + len].iter().collect();
                i += len;
                let token = match word.to_ascii_lowercase().as_str() {
                    "and" => Token::Op(Op::And),
                    "nand" => Token::Op(Op::Nand),
                    "or" => Token::Op(Op::Or),
                    "nor" => Token::Op(Op::Nor),
                    "xor" => Token::Op(Op::Xor),
                    "xnor" => Token::Op(Op::Xnor),
                    "not" => Token::Not,
                    _ => return Err(err("sub-patterns must be enclosed in quotes", column)),
                };
                tokens.push((token, column));
                continue;
            }
            _ => return Err(err("sub-patterns must be enclosed in quotes", column)),
        };
        tokens.push((token, column));
        i += 1;
    }
    Ok((tokens, patterns))
}

struct Parser<'a> {
    tokens: &'a [(Token, usize)],
    pos: usize,
    /// Column reported for errors at the end of the input.
    end: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<(&Token, usize)> {
        self.tokens.get(self.pos).map(|(t, c)| (t, *c))
    }

    fn error<T>(&self, message: &str) -> Result<T, BooleanError> {
        Err(BooleanError {
            message: message.to_owned(),
            column: self.peek().map_or(self.end, |(_, c)| c),
        })
    }

    fn binary(
        &mut self,
        ops: &[Op],
        operand: fn(&mut Self) -> Result<Expr, BooleanError>,
    ) -> Result<Expr, BooleanError> {
        let mut lhs = operand(self)?;
        while let Some((Token::Op(op), _)) = self.peek() {
            let op = *op;
            if !ops.contains(&op) {
                break;
            }
            self.pos += 1;
            let rhs = operand(self)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn or_expr(&mut self) -> Result<Expr, BooleanError> {
        self.binary(&[Op::Or, Op::Nor, Op::Xor, Op::Xnor], Self::and_expr)
    }

    fn and_expr(&mut self) -> Result<Expr, BooleanError> {
        self.binary(&[Op::And, Op::Nand], Self::unary)
    }

    fn unary(&mut self) -> Result<Expr, BooleanError> {
        match self.peek() {
            Some((Token::Not, _)) => {
                self.pos += 1;
                Ok(Expr::Not(Box::new(self.unary()?)))
            }
            Some((Token::Pattern(i), _)) => {
                let i = *i;
                self.pos += 1;
                Ok(Expr::Pattern(i))
            }
            Some((Token::Open, _)) => {
                self.pos += 1;
                let e = self.or_expr()?;
                match self.peek() {
                    Some((Token::Close, _)) => {
                        self.pos += 1;
                        Ok(e)
                    }
                    _ => self.error("expected ')'"),
                }
            }
            _ => self.error("expected a quoted pattern"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(input: &str, matched: &[bool]) -> bool {
        let (expr, patterns) = parse(input).unwrap();
        assert_eq!(patterns.len(), matched.len(), "{input}");
        expr.eval(&|i| matched[i])
    }

    fn error(input: &str) -> BooleanError {
        parse(input).unwrap_err()
    }

    #[test]
    fn sub_patterns_in_order() {
        let (_, p) = parse(r#""a" and ("b c" or "d")"#).unwrap();
        assert_eq!(p, ["a", "b c", "d"]);
    }

    #[test]
    fn escaped_quotes() {
        // klogg: "Using pattern with escaped quotes".
        let (_, p) = parse(r#""\"This\"""#).unwrap();
        assert_eq!(p, [r#""This""#]);
        let (_, p) = parse(r#""a\d+" or "\w""#).unwrap();
        assert_eq!(p, [r"a\d+", r"\w"]);
    }

    #[test]
    fn operators() {
        for (op, table) in [
            ("and", [false, false, false, true]),
            ("&", [false, false, false, true]),
            ("&&", [false, false, false, true]),
            ("nand", [true, true, true, false]),
            ("or", [false, true, true, true]),
            ("|", [false, true, true, true]),
            ("||", [false, true, true, true]),
            ("nor", [true, false, false, false]),
            ("xor", [false, true, true, false]),
            ("xnor", [true, false, false, true]),
        ] {
            for (k, expected) in table.into_iter().enumerate() {
                let (a, b) = (k & 2 != 0, k & 1 != 0);
                assert_eq!(
                    eval(&format!(r#""a" {op} "b""#), &[a, b]),
                    expected,
                    "{a} {op} {b}"
                );
            }
        }
    }

    #[test]
    fn keywords_are_case_insensitive() {
        assert!(eval(r#""a" AND Not("b")"#, &[true, false]));
    }

    #[test]
    fn not() {
        assert!(eval(r#"not("a")"#, &[false]));
        assert!(!eval(r#"!("a")"#, &[true]));
        assert!(eval(r#"!"a""#, &[false]));
        assert!(eval(r#"not(not("a"))"#, &[true]));
    }

    #[test]
    fn and_binds_tighter_than_or() {
        // a or (b and c)
        assert!(eval(r#""a" or "b" and "c""#, &[true, false, false]));
        assert!(!eval(r#"("a" or "b") and "c""#, &[true, false, false]));
        // (a and b) xor c
        assert!(eval(r#""a" and "b" xor "c""#, &[false, true, true]));
        // not applies to its operand only: (not a) and b
        assert!(eval(r#"not("a") and "b""#, &[false, true]));
    }

    #[test]
    fn left_associative() {
        // (a nand b) nand c: (T nand T) = F, F nand F = T
        assert!(eval(r#""a" nand "b" nand "c""#, &[true, true, false]));
        // (a xor b) xor c
        assert!(eval(r#""a" xor "b" xor "c""#, &[true, true, true]));
    }

    #[test]
    fn klogg_complex_patterns() {
        // klogg: "Using complex pattern with ()", sub-patterns "not_match",
        // "match", "pattern" against a line containing "match" and "pattern".
        assert!(!eval(
            r#"("not_match" | "match") & !("pattern")"#,
            &[false, true, true]
        ));
        assert!(eval(r#""not_match" | "match""#, &[false, true]));
    }

    #[test]
    fn errors() {
        assert_eq!(error("abc").message, "patterns must be enclosed in quotes");
        // klogg: "Using pattern with not matched quotes".
        assert_eq!(
            error(r#""not_match" | "match"#),
            BooleanError {
                message: "pattern has unmatched quotes".into(),
                column: 15
            }
        );
        assert_eq!(
            error(r#""a" and b"#).message,
            "sub-patterns must be enclosed in quotes"
        );
        assert_eq!(
            error(r#""a" and foo"#).message,
            "sub-patterns must be enclosed in quotes"
        );
        assert_eq!(error(r#""a" "b""#).message, "expected an operator");
        assert_eq!(error(r#""a" and"#).column, 8);
        assert_eq!(error(r#"("a""#).message, "expected ')'");
        assert_eq!(error(r#"() or "a""#).message, "expected a quoted pattern");
        assert_eq!(error(r#""a" or )"#).message, "expected a quoted pattern");
    }

    #[test]
    fn error_display() {
        assert_eq!(
            error(r#""a" and"#).to_string(),
            "expected a quoted pattern at column 8"
        );
    }
}

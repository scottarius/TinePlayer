// The `Plural-Forms` expression from a `.po` header, parsed.
//
// A catalog declares how its language pluralizes as a C expression in `n`:
//
//     nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && \
//                        (n%100<10 || n%100>=20) ? 1 : 2);
//
// polib hands that over as text and leaves the meaning to whoever wants it.
// This is that. English needs none of it - one form and a plural - but Polish
// and Russian have three and Arabic six, and a translator who cannot say so
// writes something wrong rather than something clumsy.
//
// **Used from two places, which is why it is a file of its own.** `build.rs`
// includes it to render each catalog's rule into Rust source, so the compiled
// translations carry no evaluator at all. The runtime uses it to evaluate a
// rule directly, for a `.po` loaded from disk by `TINEPLAYER_PO`, where there
// was no build to render anything. One parser, so the two cannot disagree
// about what a rule means - and `eval` below is written against `as_value`
// line for line, including the overflow choices, for the same reason.
//
// The awkward part is that C has no `bool`. `n != 1` is an integer, and both
// `plural=n>1` and `plural=n>1 ? 1 : 0` are legal whole rules. So every node
// can be asked for either reading and each wraps the other where they meet.
//
// **Plain `//` rather than `//!`, and no `#![allow]` at the top**, because
// `build.rs` reaches this file through `include!` and a macro expansion cannot
// carry inner attributes - which module documentation is one of. The lint
// exemptions are on the items instead, each with the reason it needs one.

/// One node of a parsed plural expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// The literal `n`, the count being pluralized.
    Count,
    Number(u64),
    Not(Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
}

/// Parses a `Plural-Forms` expression, or says why it cannot.
pub fn parse(expression: &str) -> Result<Expr, String> {
    let tokens = tokenize(expression)?;
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
    };
    let parsed = parser.expression()?;
    match parser.tokens.get(parser.at) {
        None => Ok(parsed),
        Some(extra) => Err(format!("unexpected `{extra}` after the end of the rule")),
    }
}

fn tokenize(expression: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let characters: Vec<char> = expression.chars().collect();
    let mut at = 0;

    while at < characters.len() {
        let character = characters[at];

        // A trailing `;` belongs to the header's own punctuation rather than to
        // the expression, and turns up with and without space around it.
        if character.is_whitespace() || character == ';' {
            at += 1;
            continue;
        }

        if character == 'n' {
            tokens.push("n".to_string());
            at += 1;
            continue;
        }

        if character.is_ascii_digit() {
            let start = at;
            while at < characters.len() && characters[at].is_ascii_digit() {
                at += 1;
            }
            tokens.push(characters[start..at].iter().collect());
            continue;
        }

        // Two-character operators first, or `<=` tokenizes as `<` then `=` and
        // the `=` is then a character no rule may contain.
        let pair: String = characters[at..(at + 2).min(characters.len())]
            .iter()
            .collect();
        if matches!(pair.as_str(), "==" | "!=" | "<=" | ">=" | "&&" | "||") {
            tokens.push(pair);
            at += 2;
            continue;
        }

        if matches!(
            character,
            '?' | ':' | '(' | ')' | '<' | '>' | '!' | '%' | '+' | '-' | '*' | '/'
        ) {
            tokens.push(character.to_string());
            at += 1;
            continue;
        }

        return Err(format!("`{character}` is not part of a plural rule"));
    }

    match tokens.is_empty() {
        true => Err("the rule is empty".to_string()),
        false => Ok(tokens),
    }
}

struct Parser<'a> {
    tokens: &'a [String],
    at: usize,
}

/// Every binary operator, loosest first, each level left-associative.
const LEVELS: &[&[&str]] = &[
    &["||"],
    &["&&"],
    &["==", "!="],
    // Longest first within a level, so `<=` is not read as `<`.
    &["<=", ">=", "<", ">"],
    &["+", "-"],
    &["*", "/", "%"],
];

impl Parser<'_> {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.at).map(String::as_str)
    }

    fn take(&mut self, token: &str) -> bool {
        match self.peek() == Some(token) {
            true => {
                self.at += 1;
                true
            }
            false => false,
        }
    }

    /// `a ? b : c`, right-associative, and the grammar's entry point.
    fn expression(&mut self) -> Result<Expr, String> {
        let condition = self.binary(0)?;
        if !self.take("?") {
            return Ok(condition);
        }
        let yes = self.expression()?;
        if !self.take(":") {
            return Err("a `?` with no `:` after it".to_string());
        }
        let no = self.expression()?;
        Ok(Expr::Ternary(
            Box::new(condition),
            Box::new(yes),
            Box::new(no),
        ))
    }

    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        let Some(operators) = LEVELS.get(level) else {
            return self.unary();
        };

        let mut left = self.binary(level + 1)?;
        loop {
            let Some(found) = operators.iter().find(|op| self.peek() == Some(**op)) else {
                return Ok(left);
            };
            self.at += 1;
            let right = self.binary(level + 1)?;
            left = Expr::Binary(found, Box::new(left), Box::new(right));
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.take("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.take("(") {
            let inner = self.expression()?;
            if !self.take(")") {
                return Err("a `(` with no `)` after it".to_string());
            }
            return Ok(inner);
        }
        match self.peek() {
            Some("n") => {
                self.at += 1;
                Ok(Expr::Count)
            }
            Some(token) if token.chars().all(|c| c.is_ascii_digit()) => {
                let number = token
                    .parse()
                    .map_err(|_| format!("`{token}` is too large for a plural rule"))?;
                self.at += 1;
                Ok(Expr::Number(number))
            }
            Some(token) => Err(format!("`{token}` where a number or `n` was expected")),
            None => Err("the rule ends in the middle of an expression".to_string()),
        }
    }
}

/// Only the rendering side asks this, so it is unused in the application.
#[allow(dead_code)]
fn is_arithmetic(operator: &str) -> bool {
    matches!(operator, "+" | "-" | "*" | "/" | "%")
}

/// Likewise.
#[allow(dead_code)]
fn is_comparison(operator: &str) -> bool {
    matches!(operator, "==" | "!=" | "<" | ">" | "<=" | ">=")
}

fn is_logical(operator: &str) -> bool {
    matches!(operator, "&&" | "||")
}

// `eval` is dead in `build.rs` and the two renderers are dead in the
// application. Neither is really dead - each is live in the other place - and
// there is no `cfg` that can tell the difference, since `include!` and `mod`
// produce the same items.
#[allow(dead_code)]
impl Expr {
    /// Which plural form `n` takes under this rule.
    ///
    /// The arithmetic here matches `as_value` exactly, overflow behavior
    /// included. A rule comes out of a file rather than out of this project,
    /// so `n - 1` at zero and `n / 0` are both reachable by someone writing
    /// them, and neither may panic: a bad rule should give a clumsy plural,
    /// not take the application down mid-film.
    pub fn eval(&self, n: u64) -> u64 {
        match self {
            Expr::Count => n,
            Expr::Number(value) => *value,
            Expr::Not(inner) => u64::from(inner.eval(n) == 0),
            Expr::Ternary(condition, yes, no) => match condition.eval(n) != 0 {
                true => yes.eval(n),
                false => no.eval(n),
            },
            Expr::Binary(operator, left, right) => {
                // `&&` and `||` short-circuit in C, and a rule may rely on it
                // to guard a division.
                if is_logical(operator) {
                    let left = left.eval(n) != 0;
                    return match *operator {
                        "&&" => u64::from(left && right.eval(n) != 0),
                        _ => u64::from(left || right.eval(n) != 0),
                    };
                }

                let (a, b) = (left.eval(n), right.eval(n));
                match *operator {
                    "==" => u64::from(a == b),
                    "!=" => u64::from(a != b),
                    "<" => u64::from(a < b),
                    ">" => u64::from(a > b),
                    "<=" => u64::from(a <= b),
                    ">=" => u64::from(a >= b),
                    "+" => a.wrapping_add(b),
                    "-" => a.saturating_sub(b),
                    "*" => a.wrapping_mul(b),
                    "/" => a.checked_div(b).unwrap_or(0),
                    _ => a.checked_rem(b).unwrap_or(0),
                }
            }
        }
    }

    /// This node as a Rust `u64` expression, which is how C reads it in value
    /// position. For `build.rs`.
    pub fn as_value(&self) -> String {
        match self {
            Expr::Count => "n".to_string(),
            Expr::Number(value) => format!("{value}u64"),
            Expr::Binary(operator, left, right) if is_arithmetic(operator) => {
                let (a, b) = (left.as_value(), right.as_value());
                match *operator {
                    "+" => format!("({a}).wrapping_add({b})"),
                    "-" => format!("({a}).saturating_sub({b})"),
                    "*" => format!("({a}).wrapping_mul({b})"),
                    "/" => format!("({a}).checked_div({b}).unwrap_or(0)"),
                    _ => format!("({a}).checked_rem({b}).unwrap_or(0)"),
                }
            }
            Expr::Ternary(condition, yes, no) => format!(
                "(if {} {{ {} }} else {{ {} }})",
                condition.as_condition(),
                yes.as_value(),
                no.as_value()
            ),
            // A comparison in value position becomes the 1 or 0 C would give.
            other => format!("(if {} {{ 1u64 }} else {{ 0u64 }})", other.as_condition()),
        }
    }

    /// This node as a Rust `bool` expression, which is how C reads it in a
    /// test. For `build.rs`.
    pub fn as_condition(&self) -> String {
        match self {
            Expr::Not(inner) => format!("(!{})", inner.as_condition()),
            Expr::Binary(operator, left, right) if is_logical(operator) => format!(
                "({} {operator} {})",
                left.as_condition(),
                right.as_condition()
            ),
            Expr::Binary(operator, left, right) if is_comparison(operator) => {
                format!("({} {operator} {})", left.as_value(), right.as_value())
            }
            Expr::Ternary(condition, yes, no) => format!(
                "(if {} {{ {} }} else {{ {} }})",
                condition.as_condition(),
                yes.as_condition(),
                no.as_condition()
            ),
            // `n` and `n % 3` alike: C calls anything non-zero true.
            other => format!("({} != 0)", other.as_value()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rules from real catalogs, against the forms they are meant to pick.
    /// Taken from the `Plural-Forms` headers gettext itself documents, and
    /// checked at the boundaries each rule turns on rather than at 1 and 2.
    #[test]
    fn real_rules_pick_the_right_forms() {
        let cases: &[(&str, &[(u64, u64)])] = &[
            // English, German, Finnish and most of Europe.
            ("n != 1", &[(0, 1), (1, 0), (2, 1), (21, 1)]),
            // French: zero goes with the singular.
            ("n > 1", &[(0, 0), (1, 0), (2, 1), (100, 1)]),
            // Japanese, Chinese, Korean, Thai: one form for everything.
            ("0", &[(0, 0), (1, 0), (99, 0)]),
            // Russian, Ukrainian, Serbian, Croatian.
            (
                "(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)",
                &[
                    (1, 0),
                    (21, 0),
                    (11, 2),
                    (111, 2),
                    (2, 1),
                    (24, 1),
                    (12, 2),
                    (5, 2),
                    (0, 2),
                ],
            ),
            // Polish.
            (
                "(n==1 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)",
                &[(1, 0), (2, 1), (22, 1), (5, 2), (12, 2), (0, 2)],
            ),
            // Arabic, the six-form case.
            (
                "n==0 ? 0 : n==1 ? 1 : n==2 ? 2 : n%100>=3 && n%100<=10 ? 3 : n%100>=11 ? 4 : 5",
                &[
                    (0, 0),
                    (1, 1),
                    (2, 2),
                    (3, 3),
                    (110, 3),
                    (11, 4),
                    (111, 4),
                    (101, 5),
                ],
            ),
            // Czech and Slovak.
            (
                "(n==1) ? 0 : (n>=2 && n<=4) ? 1 : 2",
                &[(1, 0), (2, 1), (4, 1), (5, 2), (0, 2)],
            ),
            // Irish, which is the one that uses a bare range ladder.
            (
                "n==1 ? 0 : n==2 ? 1 : n<7 ? 2 : n<11 ? 3 : 4",
                &[(1, 0), (2, 1), (6, 2), (10, 3), (11, 4)],
            ),
        ];

        for (rule, expectations) in cases {
            let parsed = parse(rule).unwrap_or_else(|e| panic!("{rule} did not parse: {e}"));
            for (n, form) in *expectations {
                assert_eq!(parsed.eval(*n), *form, "rule `{rule}` at n={n}");
            }
        }
    }

    /// The whole reason this file is shared rather than written twice: what
    /// `build.rs` renders and what the runtime evaluates have to agree. The
    /// rendering cannot be run from here - it is Rust source, and compiling it
    /// is the build's job - so what is checked is that every rule renders to
    /// something at all and that the shapes line up.
    #[test]
    fn every_rule_renders_and_evaluates() {
        for rule in [
            "n != 1",
            "n > 1",
            "0",
            "(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)",
        ] {
            let parsed = parse(rule).expect("a documented rule parses");
            assert!(!parsed.as_value().is_empty());
            assert!(!parsed.as_condition().is_empty());
            // Nothing panics anywhere in the range a count can take.
            for n in 0..200 {
                let _ = parsed.eval(n);
            }
        }
    }

    #[test]
    fn a_rule_that_makes_no_sense_is_refused() {
        for rule in ["n +", "n ? 1", "(n", "n == = 1", "", "n & 1"] {
            assert!(parse(rule).is_err(), "`{rule}` should not have parsed");
        }
    }

    /// Division by zero and subtraction below zero are both writable, and
    /// neither may bring the application down.
    #[test]
    fn arithmetic_that_would_trap_does_not() {
        assert_eq!(parse("n / 0").expect("parses").eval(5), 0);
        assert_eq!(parse("n % 0").expect("parses").eval(5), 0);
        assert_eq!(parse("n - 10").expect("parses").eval(0), 0);
    }

    /// C evaluates only one side of `&&` when the first settles it, and a rule
    /// may put a division behind that guard.
    #[test]
    fn logical_operators_short_circuit() {
        let parsed = parse("n != 0 && 10 / n > 2").expect("parses");
        assert_eq!(parsed.eval(0), 0);
        assert_eq!(parsed.eval(2), 1);
    }

    /// Precedence, which is the thing a hand-written parser gets wrong: `%`
    /// binds tighter than `==`, which binds tighter than `&&`, which binds
    /// tighter than `?:`.
    #[test]
    fn precedence_matches_c() {
        // Reads as ((n % 10) == 1), not (n % (10 == 1)).
        let parsed = parse("n % 10 == 1").expect("parses");
        assert_eq!(parsed.eval(21), 1);
        assert_eq!(parsed.eval(22), 0);

        // Reads as ((n > 1) && (n < 5)) ? 7 : 9.
        let parsed = parse("n > 1 && n < 5 ? 7 : 9").expect("parses");
        assert_eq!(parsed.eval(3), 7);
        assert_eq!(parsed.eval(9), 9);
    }
}

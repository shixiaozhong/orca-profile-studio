//! `compatible_printers_condition` / `compatible_prints_condition` expressions.
//!
//! A subset of OrcaSlicer's `PlaceholderParser` boolean expressions (`src/libslic3r/
//! PlaceholderParser.cpp`): `or`/`||`, `and`/`&&`, `not`/`!`, parentheses, `==`, `!=`/`<>`, `<`,
//! `>`, `<=`, `>=`, `=~ /regex/`, `!~ /regex/`, variables with an optional `[index]`, numbers,
//! quoted strings, `true` and `false`. Arithmetic and `if` blocks are not supported; no profile
//! uses them in a condition.

use std::fmt;

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} (at character {position})")]
pub struct ConditionError {
    pub message: String,
    /// Character offset in the expression; 0 for evaluation errors.
    pub position: usize,
}

impl ConditionError {
    fn new(message: impl Into<String>, position: usize) -> Self {
        ConditionError { message: message.into(), position }
    }
}

/// A typed value, like `PlaceholderParser`'s `expr`.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Double(f64),
    Str(String),
}

impl Value {
    fn as_f64(&self) -> Option<f64> {
        match self {
            // Values are typed by their text, so a bool option reads as 0 / 1.
            Value::Bool(value) => Some(f64::from(u8::from(*value))),
            Value::Int(value) => Some(*value as f64),
            Value::Double(value) => Some(*value),
            _ => None,
        }
    }

    fn to_text(&self) -> String {
        match self {
            Value::Bool(value) => if *value { "1" } else { "0" }.to_owned(),
            Value::Int(value) => value.to_string(),
            Value::Double(value) => value.to_string(),
            Value::Str(value) => value.clone(),
        }
    }
}

/// What a variable name resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum Variable {
    Scalar(Value),
    Vector(Vec<Value>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compare {
    Equal,
    NotEqual,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
}

#[derive(Debug, Clone)]
enum Expr {
    Literal(Value),
    Variable { name: String, index: Option<usize>, position: usize },
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Compare(Compare, Box<Expr>, Box<Expr>),
    Matches { subject: Box<Expr>, regex: Regex, negate: bool },
}

/// A parsed condition.
#[derive(Debug, Clone)]
pub struct Condition {
    source: String,
    expr: Expr,
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl Condition {
    pub fn parse(source: &str) -> Result<Condition, ConditionError> {
        let mut parser = Parser { chars: source.chars().collect(), pos: 0 };
        let expr = parser.or()?;
        parser.skip_ws();
        if parser.pos < parser.chars.len() {
            return Err(ConditionError::new("unexpected or unsupported text", parser.pos));
        }
        Ok(Condition { source: source.to_owned(), expr })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Evaluates the condition; `lookup` resolves variable names.
    pub fn evaluate(&self, lookup: &dyn Fn(&str) -> Option<Variable>) -> Result<bool, ConditionError> {
        match eval(&self.expr, lookup)? {
            Value::Bool(value) => Ok(value),
            Value::Int(value @ (0 | 1)) => Ok(value == 1),
            other => Err(ConditionError::new(format!("the condition is {other:?}, not a boolean"), 0)),
        }
    }
}

fn eval(expr: &Expr, lookup: &dyn Fn(&str) -> Option<Variable>) -> Result<Value, ConditionError> {
    let boolean = |expr: &Expr| match eval(expr, lookup)? {
        Value::Bool(value) => Ok(value),
        // A bool option is written "0" / "1" and therefore reads as an integer.
        Value::Int(value @ (0 | 1)) => Ok(value == 1),
        other => Err(ConditionError::new(format!("{other:?} is not a boolean"), 0)),
    };
    Ok(match expr {
        Expr::Literal(value) => value.clone(),
        Expr::Variable { name, index, position } => {
            let error = |message: String| ConditionError::new(message, *position);
            match (lookup(name), index) {
                (None, _) => return Err(error(format!("unknown variable `{name}`"))),
                (Some(Variable::Scalar(value)), None) => value,
                // A one-element vector option may be written as a plain string; OrcaSlicer reads
                // it as a vector, so `[0]` is that value.
                (Some(Variable::Scalar(value)), Some(0)) => value,
                (Some(Variable::Vector(values)), Some(index)) => values
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| error(format!("`{name}[{index}]` is out of range ({} items)", values.len())))?,
                // The profiles do not say which options are vectors. A single-value option written
                // as a list of equal items (`printer_notes`, `single_extruder_multi_material`) is
                // read by OrcaSlicer as that value, so an unindexed list of equal items is too.
                (Some(Variable::Vector(values)), None)
                    if values.first().is_some_and(|first| values.iter().all(|v| v == first)) =>
                {
                    values[0].clone()
                }
                (Some(Variable::Vector(_)), None) => {
                    return Err(error(format!("`{name}` is a vector; use an index such as `{name}[0]`")));
                }
                (Some(Variable::Scalar(_)), Some(_)) => return Err(error(format!("`{name}` is not a vector"))),
            }
        }
        Expr::Not(inner) => Value::Bool(!boolean(inner)?),
        // Like the C++ parser, both sides are evaluated.
        Expr::And(lhs, rhs) => Value::Bool(boolean(lhs)? & boolean(rhs)?),
        Expr::Or(lhs, rhs) => Value::Bool(boolean(lhs)? | boolean(rhs)?),
        Expr::Compare(op, lhs, rhs) => Value::Bool(compare(*op, &eval(lhs, lookup)?, &eval(rhs, lookup)?)?),
        Expr::Matches { subject, regex, negate } => match eval(subject, lookup)? {
            Value::Str(text) => Value::Bool(regex.is_match(&text) != *negate),
            other => return Err(ConditionError::new(format!("{other:?} is not a string; =~ needs one"), 0)),
        },
    })
}

/// `expr::compare_op`: numbers compare numerically (doubles within 1e-8), two booleans only for
/// equality, and anything involving a string as text.
fn compare(op: Compare, lhs: &Value, rhs: &Value) -> Result<bool, ConditionError> {
    use std::cmp::Ordering::*;
    let ordering = match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (a, b) if a.as_f64().is_some() && b.as_f64().is_some() => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            if (a - b).abs() < 1e-8 {
                Equal
            } else if a < b {
                Less
            } else {
                Greater
            }
        }
        (Value::Bool(a), Value::Bool(b)) => {
            if !matches!(op, Compare::Equal | Compare::NotEqual) {
                return Err(ConditionError::new("booleans can only be compared for equality", 0));
            }
            a.cmp(b)
        }
        (Value::Str(_), _) | (_, Value::Str(_)) => lhs.to_text().cmp(&rhs.to_text()),
        _ => return Err(ConditionError::new(format!("cannot compare {lhs:?} and {rhs:?}"), 0)),
    };
    Ok(match op {
        Compare::Equal => ordering == Equal,
        Compare::NotEqual => ordering != Equal,
        Compare::Less => ordering == Less,
        Compare::Greater => ordering == Greater,
        Compare::LessEqual => ordering != Greater,
        Compare::GreaterEqual => ordering != Less,
    })
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn skip_ws(&mut self) {
        while self.chars.get(self.pos).is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }

    /// Consumes `token` (after whitespace) if it is next. Word tokens must end at a word boundary.
    fn eat(&mut self, token: &str) -> bool {
        self.skip_ws();
        let token: Vec<char> = token.chars().collect();
        let end = self.pos + token.len();
        if self.chars.get(self.pos..end) != Some(&token[..]) {
            return false;
        }
        let word = token.iter().all(|c| c.is_alphabetic());
        if word && self.chars.get(end).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
            return false;
        }
        self.pos = end;
        true
    }

    fn or(&mut self) -> Result<Expr, ConditionError> {
        let mut lhs = self.and()?;
        while self.eat("or") || self.eat("||") {
            lhs = Expr::Or(Box::new(lhs), Box::new(self.and()?));
        }
        Ok(lhs)
    }

    fn and(&mut self) -> Result<Expr, ConditionError> {
        let mut lhs = self.comparison()?;
        while self.eat("and") || self.eat("&&") {
            lhs = Expr::And(Box::new(lhs), Box::new(self.comparison()?));
        }
        Ok(lhs)
    }

    fn comparison(&mut self) -> Result<Expr, ConditionError> {
        let lhs = self.unary()?;
        for (token, negate) in [("=~", false), ("!~", true)] {
            if self.eat(token) {
                let regex = self.regex()?;
                return Ok(Expr::Matches { subject: Box::new(lhs), regex, negate });
            }
        }
        let operators = [
            ("==", Compare::Equal),
            ("!=", Compare::NotEqual),
            ("<>", Compare::NotEqual),
            ("<=", Compare::LessEqual),
            (">=", Compare::GreaterEqual),
            ("<", Compare::Less),
            (">", Compare::Greater),
        ];
        for (token, op) in operators {
            if self.eat(token) {
                return Ok(Expr::Compare(op, Box::new(lhs), Box::new(self.unary()?)));
            }
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, ConditionError> {
        // `!` but not the start of `!=` or `!~`.
        self.skip_ws();
        let bang = self.chars.get(self.pos) == Some(&'!') && !matches!(self.chars.get(self.pos + 1), Some('=' | '~'));
        if bang {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat("not") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, ConditionError> {
        self.skip_ws();
        let start = self.pos;
        let Some(&c) = self.chars.get(self.pos) else {
            return Err(ConditionError::new("unexpected end of the expression", start));
        };
        if c == '(' {
            self.pos += 1;
            let inner = self.or()?;
            if !self.eat(")") {
                return Err(ConditionError::new("expected `)`", self.pos));
            }
            return Ok(inner);
        }
        if c == '"' {
            self.pos += 1;
            let mut text = String::new();
            loop {
                match self.chars.get(self.pos) {
                    None => return Err(ConditionError::new("unterminated string", start)),
                    Some('"') => break,
                    Some('\\') => {
                        text.extend(self.chars.get(self.pos + 1));
                        self.pos += 2;
                    }
                    Some(&c) => {
                        text.push(c);
                        self.pos += 1;
                    }
                }
            }
            self.pos += 1;
            return Ok(Expr::Literal(Value::Str(text)));
        }
        if c.is_ascii_digit() || c == '.' || c == '-' {
            return self.number();
        }
        if c.is_alphabetic() || c == '_' {
            let name = self.take_while(|c| c.is_alphanumeric() || c == '_');
            return Ok(match name.as_str() {
                "true" => Expr::Literal(Value::Bool(true)),
                "false" => Expr::Literal(Value::Bool(false)),
                _ => {
                    let index = if self.eat("[") {
                        self.skip_ws();
                        let at = self.pos;
                        let digits = self.take_while(|c| c.is_ascii_digit());
                        let index = digits.parse().map_err(|_| ConditionError::new("expected an index", at))?;
                        if !self.eat("]") {
                            return Err(ConditionError::new("expected `]`", self.pos));
                        }
                        Some(index)
                    } else {
                        None
                    };
                    Expr::Variable { name, index, position: start }
                }
            });
        }
        Err(ConditionError::new(format!("unexpected `{c}`"), start))
    }

    fn number(&mut self) -> Result<Expr, ConditionError> {
        let start = self.pos;
        let text = self.take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'));
        let value = if text.contains(['.', 'e', 'E']) {
            text.parse().map(Value::Double).ok()
        } else {
            text.parse().map(Value::Int).ok()
        };
        value.map(Expr::Literal).ok_or_else(|| ConditionError::new(format!("`{text}` is not a number"), start))
    }

    /// `/pattern/`, matched against the whole subject with Boost's defaults (`.` matches newlines).
    fn regex(&mut self) -> Result<Regex, ConditionError> {
        self.skip_ws();
        let start = self.pos;
        if self.chars.get(self.pos) != Some(&'/') {
            return Err(ConditionError::new("expected `/regex/`", start));
        }
        self.pos += 1;
        let mut pattern = String::new();
        loop {
            match self.chars.get(self.pos) {
                None => return Err(ConditionError::new("unterminated regular expression", start)),
                Some('/') => break,
                Some('\\') if self.chars.get(self.pos + 1) == Some(&'/') => {
                    pattern.push('/');
                    self.pos += 2;
                }
                Some(&c) => {
                    pattern.push(c);
                    self.pos += 1;
                }
            }
        }
        self.pos += 1;
        // boost::regex_match must match the whole string.
        Regex::new(&format!("(?s)^(?:{pattern})$"))
            .map_err(|err| ConditionError::new(format!("invalid regular expression: {err}"), start))
    }

    fn take_while(&mut self, accept: impl Fn(char) -> bool) -> String {
        let start = self.pos;
        while self.chars.get(self.pos).is_some_and(|&c| accept(c)) {
            self.pos += 1;
        }
        self.chars[start..self.pos].iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Condition, Value, Variable};

    fn lookup(name: &str) -> Option<Variable> {
        Some(match name {
            "printer_notes" => Variable::Scalar(Value::Str("Prusa\nPRINTER_MODEL_COREONE\nHF_NOZZLE".into())),
            "nozzle_diameter" => Variable::Vector(vec![Value::Double(0.6), Value::Double(0.4)]),
            "single_extruder_multi_material" => Variable::Scalar(Value::Bool(false)),
            "num_extruders" => Variable::Scalar(Value::Int(2)),
            "printer_preset" => Variable::Scalar(Value::Str("Prusa CORE One HF0.6 nozzle".into())),
            // Typed by their text, as the profiles carry no option types.
            "enable_support" => Variable::Scalar(Value::Int(1)),
            "single_nozzle" => Variable::Scalar(Value::Double(0.4)),
            "listed_flag" => Variable::Vector(vec![Value::Int(0), Value::Int(0)]),
            _ => return None,
        })
    }

    fn eval(source: &str) -> Result<bool, String> {
        Condition::parse(source).map_err(|err| err.to_string())?.evaluate(&lookup).map_err(|err| err.to_string())
    }

    #[test]
    fn evaluates_the_conditions_profiles_use() {
        let condition =
            "printer_notes=~/.*PRINTER_MODEL_COREONE.*/ and nozzle_diameter[0]==0.6 and printer_notes=~/.*HF_NOZZLE.*/";
        assert_eq!(eval(condition), Ok(true));
        assert_eq!(eval("printer_notes=~/.*MK4S.*/ and nozzle_diameter[0]==0.6"), Ok(false));
        assert_eq!(eval("printer_notes!~/.*HF_NOZZLE.*/"), Ok(false));
        assert_eq!(eval("nozzle_diameter[0]==0.4 and single_extruder_multi_material"), Ok(false));
        // A full match: the pattern must cover the whole value, across newlines.
        assert_eq!(eval("printer_notes=~/PRINTER_MODEL_COREONE/"), Ok(false));
    }

    #[test]
    fn supports_the_rest_of_the_boolean_grammar() {
        assert_eq!(eval("not (num_extruders > 1) || nozzle_diameter[1] <= 0.4"), Ok(true));
        assert_eq!(eval("!single_extruder_multi_material && num_extruders <> 1"), Ok(true));
        assert_eq!(eval("printer_preset == \"Prusa CORE One HF0.6 nozzle\""), Ok(true));
        assert_eq!(eval("num_extruders == 2.0 and true"), Ok(true));
        assert_eq!(eval("nozzle_diameter[0] >= 0.6 and nozzle_diameter[0] != 0.6"), Ok(false));
        // Words are not split: `order` is not `or` + `der`.
        assert_eq!(eval("ordered == 1").unwrap_err(), "unknown variable `ordered` (at character 0)");
    }

    #[test]
    fn values_typed_by_their_text_still_compare_like_orca() {
        // A bool option written "1" reads as an integer and still acts as a boolean.
        assert_eq!(eval("enable_support"), Ok(true));
        assert_eq!(eval("enable_support and single_extruder_multi_material == 0"), Ok(true));
        // A one-element vector written as a plain string is indexable at 0.
        assert_eq!(eval("single_nozzle[0] == 0.4"), Ok(true));
        assert!(eval("single_nozzle[1] == 0.4").unwrap_err().contains("not a vector"));
        // A single value written as a list of equal items reads as that value.
        assert_eq!(eval("listed_flag == 0"), Ok(true));
    }

    #[test]
    fn reports_parse_and_evaluation_errors() {
        assert!(eval("nozzle_diameter[0] + 1 == 2").unwrap_err().contains("unsupported"));
        assert!(eval("printer_notes =~ /(/").unwrap_err().contains("invalid regular expression"));
        assert!(eval("(num_extruders == 2").unwrap_err().contains("expected `)`"));
        assert!(eval("nozzle_diameter == 0.4").unwrap_err().contains("is a vector"));
        assert!(eval("nozzle_diameter[5] == 0.4").unwrap_err().contains("out of range"));
        assert!(eval("num_extruders").unwrap_err().contains("not a boolean"));
    }
}

//! The one place a value reaches SQL (#310, D-027).
//!
//! - [`Params`] binds a value to a named placeholder (`$p0`, `$p1`…) that
//!   the query crate fills in with DataFusion's parameter values: the value
//!   is never part of the SQL text.
//! - [`quote`] makes a string literal, for the few places a placeholder
//!   cannot go: inside a view (the feature map's projection is one, and
//!   DataFusion does not fill placeholders inside views) and a cast's type.
//! - [`checked`] is the rule every value from a request goes through first:
//!   not empty, at most [`MAX_VALUE`] bytes, no control character.
//!
//! `cargo xtask ci` fails when quotes are doubled anywhere else.

use std::collections::BTreeMap;
use std::fmt;

/// The longest value a filter may hold.
pub const MAX_VALUE: usize = 200;

/// A value refused by [`checked`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid;

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("empty, too long or with a control character")
    }
}

impl std::error::Error for Invalid {}

/// `v` if a request may use it as a SQL value.
pub fn checked(v: &str) -> Result<&str, Invalid> {
    if v.is_empty() || v.len() > MAX_VALUE || v.chars().any(char::is_control) {
        Err(Invalid)
    } else {
        Ok(v)
    }
}

/// `v` as a SQL string literal: quotes doubled, nothing else changed.
pub fn quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}

/// A [`checked`] value as a SQL string literal.
pub fn lit(v: &str) -> Result<String, Invalid> {
    checked(v).map(quote)
}

/// Named values for one query, filled in by DataFusion (never spliced
/// into the text).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Params(BTreeMap<String, String>);

impl Params {
    /// No values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind a [`checked`] value and return its placeholder (`$p0`…). The
    /// same value bound twice shares one placeholder.
    pub fn bind(&mut self, v: &str) -> Result<String, Invalid> {
        let v = checked(v)?;
        if let Some((k, _)) = self.0.iter().find(|(_, x)| x.as_str() == v) {
            return Ok(format!("${k}"));
        }
        let k = format!("p{}", self.0.len());
        self.0.insert(k.clone(), v.to_owned());
        Ok(format!("${k}"))
    }

    /// Bind a [`checked`] value to a fixed name (ASCII letters and `_`),
    /// for a clause written once with `$name` in it. Returns `$name`.
    pub fn set(&mut self, name: &str, v: &str) -> Result<String, Invalid> {
        let v = checked(v)?;
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
            return Err(Invalid);
        }
        self.0.insert(name.to_owned(), v.to_owned());
        Ok(format!("${name}"))
    }

    /// Name (without `$`) and value of every binding.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Whether nothing is bound.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Both sets of bindings. Names are taken from the left: a value
    /// already bound keeps its placeholder, and `other` must not have been
    /// used in a query text yet.
    pub fn merge(&mut self, other: &Params) -> Result<(), Invalid> {
        for (_, v) in other.iter() {
            self.bind(v)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_bounded_values_pass() {
        for bad in ["", "a\nb", "\u{0}", &"x".repeat(MAX_VALUE + 1)] {
            assert_eq!(checked(bad), Err(Invalid), "{bad:?}");
            assert!(lit(bad).is_err());
        }
        assert_eq!(lit("o'clock").unwrap(), "'o''clock'");
        assert_eq!(quote("a''b"), "'a''''b'");
        assert_eq!(lit("ünï ✓").unwrap(), "'ünï ✓'");
    }

    #[test]
    fn values_get_placeholders_not_text() {
        let mut p = Params::new();
        let a = p.bind("'; DROP TABLE x; --").unwrap();
        let b = p.bind("admin").unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("$p0", "$p1"));
        assert_eq!(p.bind("admin").unwrap(), "$p1");
        assert!(p.bind("").is_err());
        let got: Vec<_> = p.iter().collect();
        assert_eq!(got, [("p0", "'; DROP TABLE x; --"), ("p1", "admin")]);
    }
}

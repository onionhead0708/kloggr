//! Search patterns and their compiled matchers.
//!
//! Port of klogg's `RegularExpressionPattern`, `RegularExpression` and
//! `PatternMatcher` (`src/regex/`, GPLv3). klogg matches with Hyperscan or
//! QRegularExpression; kloggr uses the `regex` crate, whose syntax is close to
//! PCRE minus look-around and backreferences.

use std::ops::Range;

use regex::bytes::{Regex, RegexBuilder};

use super::boolean::{self, BooleanError, Expr};

/// What to search for (klogg: `RegularExpressionPattern`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub text: String,
    pub case_sensitive: bool,
    /// Match the lines that do *not* match (klogg: `isExclude`, "inverse").
    pub exclude: bool,
    /// `text` is a boolean combination of quoted sub-patterns (see
    /// [`boolean`](super::boolean)).
    pub boolean: bool,
    /// `text`, or each sub-pattern, is literal text rather than a regex.
    pub plain_text: bool,
}

impl Pattern {
    /// A case-sensitive regex.
    pub fn regex(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            case_sensitive: true,
            exclude: false,
            boolean: false,
            plain_text: false,
        }
    }

    /// A case-sensitive plain-text pattern.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            plain_text: true,
            ..Self::regex(text)
        }
    }

    pub fn case_insensitive(self) -> Self {
        Self {
            case_sensitive: false,
            ..self
        }
    }

    pub fn excluding(self) -> Self {
        Self {
            exclude: true,
            ..self
        }
    }

    pub fn boolean(self) -> Self {
        Self {
            boolean: true,
            ..self
        }
    }
}

/// Why a pattern can't be compiled. The message is meant for the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    #[error("{0}")]
    Regex(String),
    #[error("{0}")]
    Boolean(#[from] BooleanError),
}

impl From<regex::Error> for PatternError {
    fn from(e: regex::Error) -> Self {
        PatternError::Regex(e.to_string())
    }
}

#[derive(Debug, Clone)]
enum Kind {
    Single(Regex),
    /// Sub-patterns are matched lazily as the expression is evaluated, so
    /// short-circuiting skips them (each appears once in the expression).
    /// This beats a `RegexSet`, which always runs all of them.
    Boolean {
        regexes: Vec<Regex>,
        expr: Expr,
    },
}

/// Matches found over a block before [`Matcher::match_block`] checks
/// whether they are dense enough to test lines one by one instead.
const DENSE_AFTER: usize = 32;

/// A compiled [`Pattern`] (klogg: `PatternMatcher`). Cheap to share between
/// threads.
#[derive(Debug, Clone)]
pub struct Matcher {
    kind: Kind,
    exclude: bool,
    /// The single pattern with `^` and `$` matching at line breaks, to run
    /// over many lines at once (see [`match_block`](Self::match_block));
    /// `None` when that could select other lines than matching line by line.
    block: Option<Regex>,
}

impl Matcher {
    pub fn new(pattern: &Pattern) -> Result<Self, PatternError> {
        let source = |text: &str| {
            if pattern.plain_text {
                regex::escape(text)
            } else {
                text.to_owned()
            }
        };
        let mut block = None;
        let kind = if pattern.boolean {
            let (expr, subs) = boolean::parse(&pattern.text)?;
            let subs: Vec<String> = subs.iter().map(|s| source(s)).collect();
            let regexes = subs
                .iter()
                .map(|s| build(s, pattern.case_sensitive))
                .collect::<Result<_, _>>()?;
            Kind::Boolean { regexes, expr }
        } else {
            let source = source(&pattern.text);
            // `\A` and `\z` mean the line's start and end only when lines
            // are matched one at a time, and `(?-m)` turns `^`/`$` back into
            // them. (An escaped backslash before A or z is caught too: a
            // pattern only loses the faster path.)
            let line_anchored = ["\\A", "\\z", "-m"].iter().any(|a| source.contains(a));
            if !line_anchored {
                block = RegexBuilder::new(&source)
                    .case_insensitive(!pattern.case_sensitive)
                    .multi_line(true)
                    .build()
                    .ok();
            }
            Kind::Single(build(&source, pattern.case_sensitive)?)
        };
        Ok(Self {
            kind,
            exclude: pattern.exclude,
            block,
        })
    }

    /// The lines of a block of UTF-8 text (`bounds`: each line's start in
    /// `data`, then the end of the last one) that the pattern selects, as
    /// indexes from the block's first line. The regex runs over the whole
    /// block instead of once per line, which is what short lines cost; a
    /// line is checked on its own only when a match runs past its end.
    /// `None` when the block must be searched line by line: a boolean
    /// pattern, an anchor that means the line's start or end, or a carriage
    /// return in the block (lines are compared without it).
    pub fn match_block(&self, data: &[u8], bounds: &[usize]) -> Option<Vec<usize>> {
        let (Kind::Single(line_re), Some(block_re)) = (&self.kind, &self.block) else {
            return None;
        };
        if memchr::memchr(b'\r', data).is_some() {
            return None;
        }
        let lines = bounds.len().saturating_sub(1);
        // A file truncated under a read gives fewer bytes than the index.
        let end_of = |k: usize| bounds[k + 1].min(data.len());
        let content_end = |k: usize| {
            let end = end_of(k);
            if end > bounds[k] && data[end - 1] == b'\n' {
                end - 1
            } else {
                end
            }
        };
        let mut matched = Vec::new();
        let mut at = 0;
        while let Some(m) = block_re.find_at(data, at) {
            // The line the match starts in.
            let k = bounds
                .partition_point(|&b| b <= m.start())
                .saturating_sub(1);
            if k >= lines {
                break;
            }
            let end = content_end(k);
            let within = m.end() <= end;
            let start = bounds[k].min(end);
            if within || line_re.is_match(&data[start..end]) {
                matched.push(k);
            }
            let next = end_of(k);
            if next <= at {
                break;
            }
            at = next;
            // Most lines match: a search per match costs more than testing
            // each line, so test the rest of the block line by line.
            if matched.len() >= DENSE_AFTER && matched.len() * 4 > k + 1 {
                matched.extend((k + 1..lines).filter(|&j| {
                    line_re.is_match(&data[bounds[j].min(content_end(j))..content_end(j)])
                }));
                break;
            }
        }
        if self.exclude {
            let mut matched = matched.into_iter().peekable();
            return Some(
                (0..lines)
                    .filter(|k| {
                        if matched.peek() == Some(k) {
                            matched.next();
                            false
                        } else {
                            true
                        }
                    })
                    .collect(),
            );
        }
        Some(matched)
    }

    /// Whether `line` (UTF-8, without terminator) is selected by the pattern.
    pub fn is_match(&self, line: &[u8]) -> bool {
        let matched = match &self.kind {
            Kind::Single(re) => re.is_match(line),
            Kind::Boolean { regexes, expr } => expr.eval(&|i| regexes[i].is_match(line)),
        };
        matched != self.exclude
    }

    /// Byte ranges of the pattern's matches in `line`, sorted and merged, to
    /// highlight them. Empty for an excluding pattern. For a boolean pattern,
    /// the matches of every sub-pattern, whether or not the line is selected.
    pub fn match_ranges(&self, line: &str) -> Vec<Range<usize>> {
        if self.exclude {
            return Vec::new();
        }
        let line = line.as_bytes();
        let mut ranges: Vec<Range<usize>> = match &self.kind {
            Kind::Single(re) => non_empty_matches(re, line).collect(),
            Kind::Boolean { regexes, .. } => regexes
                .iter()
                .flat_map(|re| non_empty_matches(re, line))
                .collect(),
        };
        ranges.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for r in ranges {
            match merged.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
        }
        merged
    }
}

fn build(source: &str, case_sensitive: bool) -> Result<Regex, regex::Error> {
    RegexBuilder::new(source)
        .case_insensitive(!case_sensitive)
        .build()
}

fn non_empty_matches<'a>(re: &'a Regex, line: &'a [u8]) -> impl Iterator<Item = Range<usize>> + 'a {
    re.find_iter(line)
        .map(|m| m.range())
        .filter(|r| !r.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: Pattern, line: &str) -> bool {
        Matcher::new(&pattern).unwrap().is_match(line.as_bytes())
    }

    const KLOGG_LINE: &str = r#""This" is matching pattern"#;

    #[test]
    fn regex() {
        assert!(matches(
            Pattern::regex(r"Creat(ed|ing) \d+"),
            "Created 42 items"
        ));
        assert!(!matches(
            Pattern::regex(r"Creat(ed|ing) \d+"),
            "Created items"
        ));
        assert!(matches(Pattern::regex("^abc$"), "abc"));
        assert!(!matches(Pattern::regex("^abc$"), "xabc"));
    }

    #[test]
    fn case() {
        assert!(!matches(Pattern::regex("error"), "ERROR: x"));
        assert!(matches(
            Pattern::regex("error").case_insensitive(),
            "ERROR: x"
        ));
        assert!(matches(
            Pattern::regex("straße").case_insensitive(),
            "STRASSE straße"
        ));
        assert!(matches(Pattern::regex("ÉTÉ").case_insensitive(), "été"));
    }

    #[test]
    fn plain_text_is_escaped() {
        assert!(matches(Pattern::plain("a.b(c)"), "x a.b(c) y"));
        assert!(!matches(Pattern::plain("a.b"), "axb"));
        assert!(matches(Pattern::regex("a.b"), "axb"));
        assert!(matches(
            Pattern::plain("[ERROR]").case_insensitive(),
            "[error] x"
        ));
    }

    #[test]
    fn exclude() {
        assert!(!matches(Pattern::plain("debug").excluding(), "debug: x"));
        assert!(matches(Pattern::plain("debug").excluding(), "info: x"));
    }

    #[test]
    fn empty_pattern_matches_everything() {
        assert!(matches(Pattern::regex(""), ""));
        assert!(matches(Pattern::regex(""), "abc"));
    }

    #[test]
    fn invalid_utf8_lines() {
        assert!(
            Matcher::new(&Pattern::regex("b+"))
                .unwrap()
                .is_match(b"a\xffbb")
        );
    }

    // klogg tests/unit/patternmatcher_test.cpp, "Pattern matcher in boolean mode".

    #[test]
    fn klogg_boolean_single_pattern() {
        let p = Pattern::plain(r#""matching""#).case_insensitive().boolean();
        assert!(matches(p, KLOGG_LINE));
    }

    #[test]
    fn klogg_boolean_complex_pattern() {
        let p = Pattern::plain(r#""not_match" | "match""#)
            .case_insensitive()
            .boolean();
        assert!(matches(p, KLOGG_LINE));
    }

    #[test]
    fn klogg_boolean_complex_pattern_with_parentheses() {
        let p = Pattern::regex(r#"("not_match" | "match") & !("pattern")"#)
            .case_insensitive()
            .boolean();
        assert!(!matches(p, KLOGG_LINE));
    }

    #[test]
    fn klogg_boolean_escaped_quotes() {
        let p = Pattern::regex(r#""\"This\"""#).case_insensitive().boolean();
        assert!(matches(p, KLOGG_LINE));
    }

    #[test]
    fn klogg_boolean_unmatched_quotes() {
        let p = Pattern::regex(r#""not_match" | "match"#)
            .case_insensitive()
            .boolean();
        assert!(matches!(Matcher::new(&p), Err(PatternError::Boolean(_))));
    }

    #[test]
    fn boolean_with_regex_sub_patterns() {
        let p = Pattern::regex(r#""^\d{4}-" and not("DEBUG|TRACE")"#).boolean();
        assert!(matches(p.clone(), "2024-01-01 ERROR x"));
        assert!(!matches(p.clone(), "2024-01-01 DEBUG x"));
        assert!(!matches(p, "ERROR x"));
    }

    #[test]
    fn boolean_exclude() {
        let p = Pattern::plain(r#""a" and "b""#).boolean().excluding();
        assert!(!matches(p.clone(), "a b"));
        assert!(matches(p, "a"));
    }

    #[test]
    fn invalid_regex() {
        let e = Matcher::new(&Pattern::regex("a(b")).unwrap_err();
        assert!(matches!(e, PatternError::Regex(ref m) if m.contains("unclosed group")));
        let e = Matcher::new(&Pattern::regex(r#""ok" or "(""#).boolean()).unwrap_err();
        assert!(matches!(e, PatternError::Regex(_)));
    }

    /// The block path selects exactly the lines matching line by line does.
    #[test]
    fn block_matches_like_lines() {
        let text = "alpha beta\n\ngamma alpha\nalpha\n  x  \nbeta gamma\nlast alpha";
        let mut bounds = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                bounds.push(i + 1);
            }
        }
        bounds.push(text.len());
        let lines: Vec<&str> = text.split('\n').collect();
        let patterns = [
            Pattern::regex("alpha"),
            Pattern::regex("^alpha"),
            Pattern::regex("alpha$"),
            Pattern::regex("^$"),
            Pattern::regex(r"\s+"),
            Pattern::regex(r"a\s*\n?g"),
            Pattern::regex(r"(?s)beta.gamma"),
            Pattern::regex(r"\bgamma\b"),
            Pattern::regex("x*"),
            Pattern::regex(""),
            Pattern::regex(r"[^a-z ]"),
            Pattern::plain("a b").case_insensitive(),
            Pattern::regex("ALPHA").case_insensitive(),
            Pattern::regex("alpha").excluding(),
            Pattern::regex("^$").excluding(),
        ];
        for pattern in patterns {
            let m = Matcher::new(&pattern).unwrap();
            let by_line: Vec<usize> = (0..lines.len())
                .filter(|&k| m.is_match(lines[k].as_bytes()))
                .collect();
            let by_block = m.match_block(text.as_bytes(), &bounds).unwrap();
            assert_eq!(by_block, by_line, "{pattern:?}");
        }
        // Line by line when the block can't tell.
        for pattern in [Pattern::regex(r"\Aalpha"), Pattern::regex(r"(?-m)^alpha")] {
            let m = Matcher::new(&pattern).unwrap();
            assert_eq!(m.match_block(text.as_bytes(), &bounds), None, "{pattern:?}");
        }
        let boolean = Matcher::new(&Pattern::regex(r#""a""#).boolean()).unwrap();
        assert_eq!(boolean.match_block(text.as_bytes(), &bounds), None);
        let crlf = "a\r\nb\n";
        let m = Matcher::new(&Pattern::regex("a$")).unwrap();
        assert_eq!(m.match_block(crlf.as_bytes(), &[0, 3, 5]), None);
        // Dense matches switch to testing each line, with the same result.
        let dense: String = (0..200).map(|i| format!("v{}\n", i % 3)).collect();
        let mut bounds = vec![0];
        bounds.extend(dense.match_indices('\n').map(|(i, _)| i + 1));
        for pattern in [Pattern::regex("v[01]"), Pattern::regex("v[01]").excluding()] {
            let m = Matcher::new(&pattern).unwrap();
            let by_line: Vec<usize> = (0..200)
                .filter(|&k| m.is_match(format!("v{}", k % 3).as_bytes()))
                .collect();
            assert_eq!(m.match_block(dense.as_bytes(), &bounds).unwrap(), by_line);
        }
        // Fewer bytes than the index says (a truncated file being read).
        let m = Matcher::new(&Pattern::regex("b")).unwrap();
        assert_eq!(m.match_block(b"ab\nb", &[0, 3, 6, 9]), Some(vec![0, 1]));
    }

    #[test]
    fn match_ranges() {
        let m = Matcher::new(&Pattern::regex(r"\d+")).unwrap();
        assert_eq!(m.match_ranges("a 12 b 345"), [2..4, 7..10]);
        // Empty matches are not highlighted.
        let m = Matcher::new(&Pattern::regex(r"x*")).unwrap();
        assert_eq!(m.match_ranges("axxb"), vec![1..3]);
        let m = Matcher::new(&Pattern::plain("b").excluding()).unwrap();
        assert!(m.match_ranges("abc").is_empty());
    }

    #[test]
    fn match_ranges_boolean_merges_overlaps() {
        let m = Matcher::new(&Pattern::regex(r#""abc" or "bcd" or "z""#).boolean()).unwrap();
        assert_eq!(m.match_ranges("xabcde z"), [1..5, 7..8]);
        // Sub-patterns are highlighted even under `not`.
        let m = Matcher::new(&Pattern::plain(r#""a" and not("c")"#).boolean()).unwrap();
        assert_eq!(m.match_ranges("abc"), [0..1, 2..3]);
    }
}

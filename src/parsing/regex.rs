use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize, Serializer, Deserializer};
use serde_bytes::{Bytes, ByteBuf};
use std::error::Error;
use std::sync::Arc;

use crate::dumps::dump_to_uncompressed_binary;

/// An abstraction for regex patterns.
///
/// * Allows swapping out the regex implementation because it's only in this module.
/// * Makes regexes serializable and deserializable using just the pattern string.
/// * Lazily compiles regexes on first use to improve initialization time.
#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Regex {
    source: Arc<RegexSource>,
    #[serde(skip)]
    regex: OnceCell<regex_impl::Regex>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RegexSource {
    Pattern(String),
    Binary(Vec<u8>),
    ExprTree(fancy_regex::ExprTree),
}

/// A region contains text positions for capture groups in a match result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Region {
    region: regex_impl::Region,
}

impl Regex {
    /// Create a new regex from the pattern string.
    ///
    /// Note that the regex compilation happens on first use, which is why this method does not
    /// return a result.
    pub fn new(pattern: String) -> Self {
        Self {
            source: Arc::new(RegexSource::Pattern(pattern)),
            regex: OnceCell::new(),
        }
    }

    /// Deserialize a new regex from the bytes.
    pub fn deserialize(binary: Vec<u8>) -> Self {
        Self {
            source: Arc::new(RegexSource::Binary(binary)),
            regex: OnceCell::new(),
        }
    }

    /// Create a new regex from the expr tree.
    pub fn from_expr_tree(tree: fancy_regex::ExprTree) -> Self {
        Self {
            source: Arc::new(RegexSource::ExprTree(tree)),
            regex: OnceCell::new(),
        }
    }

    /// Check whether the pattern compiles as a valid regex or not.
    pub fn try_compile(regex_str: &str) -> Option<Box<dyn Error + Send + Sync + 'static>> {
        regex_impl::Regex::parse_expr_tree(regex_str).err()
    }

    /// Get expr tree
    pub fn expr_tree(&self) -> Result<fancy_regex::ExprTree, Box<dyn Error + Send + Sync + 'static>> {
        match self.source.as_ref() {
            RegexSource::Pattern(pattern) => regex_impl::Regex::parse_expr_tree(pattern),
            RegexSource::Binary(binary) => regex_impl::Regex::deserialize_expr_tree(binary),
            RegexSource::ExprTree(tree) => Ok(tree.clone())
        }
    }

    /// Check if the regex matches the given text.
    pub fn is_match(&self, text: &str) -> bool {
        self.regex().is_match(text)
    }

    /// Search for the pattern in the given text from begin/end positions.
    ///
    /// If a region is passed, it is used for storing match group positions. The argument allows
    /// the [`Region`] to be reused between searches, which makes a significant performance
    /// difference.
    ///
    /// [`Region`]: struct.Region.html
    pub fn search(
        &self,
        text: &str,
        begin: usize,
        end: usize,
        region: Option<&mut Region>,
    ) -> bool {
        self.regex()
            .search(text, begin, end, region.map(|r| &mut r.region))
    }

    fn regex(&self) -> &regex_impl::Regex {
        self.regex.get_or_init(|| {
            regex_impl::Regex::from_expr_tree(self.expr_tree().expect("regex string should be pre-tested"))
        })
    }
}

impl Clone for Regex {
    fn clone(&self) -> Self {
        Regex {
            source: self.source.clone(),
            regex: OnceCell::new(),
        }
    }
}

impl PartialEq for Regex {
    fn eq(&self, other: &Regex) -> bool {
        self.source == other.source
    }
}

impl Eq for Regex {}


impl Serialize for RegexSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            RegexSource::Binary(binary) => Bytes::new(binary.as_slice()).serialize(serializer),
            RegexSource::ExprTree(tree) => ByteBuf::from(dump_to_uncompressed_binary(tree)).serialize(serializer),
            RegexSource::Pattern(pattern) => ByteBuf::from(dump_to_uncompressed_binary(&regex_impl::Regex::parse_expr_tree(pattern).map_err(|_| serde::ser::Error::custom("invalid regex"))?)).serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for RegexSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(RegexSource::Binary(ByteBuf::deserialize(deserializer)?.into_vec()))
    }
}

impl Region {
    pub fn new() -> Self {
        Self {
            region: regex_impl::new_region(),
        }
    }

    /// Get the start/end positions of the capture group with given index.
    ///
    /// If there is no match for that group or the index does not correspond to a group, `None` is
    /// returned. The index 0 returns the whole match.
    pub fn pos(&self, index: usize) -> Option<(usize, usize)> {
        self.region.pos(index)
    }
}

impl Default for Region {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "regex-onig")]
mod regex_impl {
    pub use onig::Region;
    use onig::{MatchParam, RegexOptions, SearchOptions, Syntax};
    use std::error::Error;

    #[derive(Debug)]
    pub struct Regex {
        regex: onig::Regex,
    }

    pub fn new_region() -> Region {
        Region::with_capacity(8)
    }

    impl Regex {
        pub fn new(regex_str: &str) -> Result<Regex, Box<dyn Error + Send + Sync + 'static>> {
            let result = onig::Regex::with_options(
                regex_str,
                RegexOptions::REGEX_OPTION_CAPTURE_GROUP,
                Syntax::default(),
            );
            match result {
                Ok(regex) => Ok(Regex { regex }),
                Err(error) => Err(Box::new(error)),
            }
        }

        pub fn is_match(&self, text: &str) -> bool {
            self.regex
                .match_with_options(text, 0, SearchOptions::SEARCH_OPTION_NONE, None)
                .is_some()
        }

        pub fn search(
            &self,
            text: &str,
            begin: usize,
            end: usize,
            region: Option<&mut Region>,
        ) -> bool {
            let matched = self.regex.search_with_param(
                text,
                begin,
                end,
                SearchOptions::SEARCH_OPTION_NONE,
                region,
                MatchParam::default(),
            );

            // If there's an error during search, treat it as non-matching.
            // For example, in case of catastrophic backtracking, onig should
            // fail with a "retry-limit-in-match over" error eventually.
            matches!(matched, Ok(Some(_)))
        }
    }
}

// If both regex-fancy and regex-onig are requested, this condition makes regex-onig win.
#[cfg(all(feature = "regex-fancy", not(feature = "regex-onig")))]
mod regex_impl {
    use std::error::Error;

    use smallvec::SmallVec;

    use crate::dumps::from_uncompressed_data;

    #[derive(Debug)]
    pub struct Regex {
        regex: fancy_regex::Regex,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Region {
        positions: SmallVec<[Option<(usize, usize)>; 8]>,
    }

    pub fn new_region() -> Region {
        Region {
            positions: SmallVec::new(),
        }
    }

    impl Regex {
        pub fn parse_expr_tree(pattern: &str) -> Result<fancy_regex::ExprTree, Box<dyn Error + Send + Sync + 'static>> {
            Ok(fancy_regex::Expr::parse_tree(pattern)?)
        }

        pub fn deserialize_expr_tree(binary: &[u8]) -> Result<fancy_regex::ExprTree, Box<dyn Error + Send + Sync + 'static>> {
            Ok(from_uncompressed_data(binary)?)
        }

        pub fn from_expr_tree(expr_tree: fancy_regex::ExprTree) -> Regex {
            let regex = fancy_regex::RegexBuilder::new().build_from_expr_tree(expr_tree).unwrap();
            Regex { regex }
        }

        pub fn is_match(&self, text: &str) -> bool {
            // Errors are treated as non-matches
            self.regex.is_match(text).unwrap_or(false)
        }

        pub fn search(
            &self,
            text: &str,
            begin: usize,
            end: usize,
            region: Option<&mut Region>,
        ) -> bool {
            // If there's an error during search, treat it as non-matching.
            // For example, in case of catastrophic backtracking, fancy-regex should
            // fail with an error eventually.
            if let Ok(Some(captures)) = self.regex.captures_from_pos(&text[..end], begin) {
                if let Some(region) = region {
                    region.init_from_captures(&captures);
                }
                true
            } else {
                false
            }
        }
    }

    impl Region {
        fn init_from_captures(&mut self, captures: &fancy_regex::Captures) {
            self.positions.clear();
            for i in 0..captures.len() {
                let pos = captures.get(i).map(|m| (m.start(), m.end()));
                self.positions.push(pos);
            }
        }

        pub fn pos(&self, i: usize) -> Option<(usize, usize)> {
            if i < self.positions.len() {
                self.positions[i]
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_compiled_regex() {
        let regex = Regex::new(String::from(r"\w+"));

        assert!(regex.regex.get().is_none());
        assert!(regex.is_match("test"));
        assert!(regex.regex.get().is_some());
    }
}

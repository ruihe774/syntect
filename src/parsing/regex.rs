use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize, Serializer, Deserializer};
use smallvec::SmallVec;
use std::sync::RwLock;

/// An abstraction for regex patterns.
#[derive(Debug)]
pub struct Regex {
    tree: RwLock<Option<fancy_regex::ExprTree>>,
    regex: OnceCell<fancy_regex::Regex>,
}

impl Regex {
    /// Create a new regex from the pattern string.
    pub fn from_pattern(pattern: &str) -> fancy_regex::Result<Self> {
        let tree = fancy_regex::Expr::parse_tree(&pattern)?;
        Ok(Regex {
            tree: RwLock::new(Some(tree)),
            regex: OnceCell::new(),
        })
    }

    /// Create a new regex from the expr tree.
    pub fn from_expr_tree(tree: fancy_regex::ExprTree) ->Self {
        Regex {
            tree: RwLock::new(Some(tree)),
            regex: OnceCell::new(),
        }
    }

    /// Get expr tree
    pub fn get_expr_tree(&self) -> fancy_regex::ExprTree {
        let tree_ref = self.tree.read().unwrap();
        if let Some(tree) = &*tree_ref {
            debug_assert!(self.regex.get().is_none());
            tree.clone()
        } else {
            self.regex.get().unwrap().as_expr_tree().clone()
        }
    }

    /// Check if the regex matches the given text.
    pub fn is_match(&self, text: &str) -> bool {
        self.regex().is_match(text).unwrap_or_default()
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
        if let Ok(Some(captures)) = self.regex().captures_within_range(text, begin..end) {
            if let Some(region) = region {
                region.init_from_captures(&captures);
            }
            true
        } else {
            false
        }
    }

    fn regex(&self) -> &fancy_regex::Regex {
        self.regex.get_or_init(|| {
            fancy_regex::RegexBuilder::new().build_from_expr_tree(self.tree.write().unwrap().take().unwrap()).expect("regex string should be pre-tested")
        })
    }
}

impl Clone for Regex {
    fn clone(&self) -> Self {
        let tree_ref = self.tree.read().unwrap();
        if let Some(tree) = &*tree_ref {
            debug_assert!(self.regex.get().is_none());
            Regex {
                tree: RwLock::new(Some(tree.clone())),
                regex: OnceCell::new(),
            }
        } else {
            Regex {
                tree: RwLock::new(None),
                regex: OnceCell::from(self.regex.get().unwrap().clone()),
            }
        }
    }
}

impl PartialEq for Regex {
    fn eq(&self, other: &Regex) -> bool {
        let left_borrow = self.tree.read().unwrap();
        let left_expr = left_borrow.as_ref().unwrap_or_else(|| self.regex.get().unwrap().as_expr_tree());
        let right_borrow = other.tree.read().unwrap();
        let right_expr = right_borrow.as_ref().unwrap_or_else(|| other.regex.get().unwrap().as_expr_tree());
        left_expr == right_expr
    }
}

impl Eq for Regex {}

impl Serialize for Regex {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let borrow = self.tree.read().unwrap();
        let expr = borrow.as_ref().unwrap_or_else(|| self.regex.get().unwrap().as_expr_tree());
        expr.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Regex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Regex::from_expr_tree(Deserialize::deserialize(deserializer)?))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Region {
    positions: SmallVec<[Option<(usize, usize)>; 8]>,
}

impl Region {
    pub fn new() -> Self {
        Self::default()
    }

    fn init_from_captures(&mut self, captures: &fancy_regex::Captures) {
        self.positions.clear();
        self.positions.extend((0..captures.len()).map(|i| captures.get(i).map(|m| (m.start(), m.end()))));
    }

    pub fn pos(&self, i: usize) -> Option<(usize, usize)> {
        if i < self.positions.len() {
            self.positions[i]
        } else {
            None
        }
    }
}

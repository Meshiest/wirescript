use std::{borrow::Borrow, fmt::Display, ops::Deref, sync::Arc};

/// A string that can be owned, static, or shared.
#[derive(Debug, Clone)]
pub enum BString {
    Owned(String),
    Static(&'static str),
    Arc(Arc<String>),
}

impl Eq for BString {}
impl PartialEq for BString {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}
impl std::hash::Hash for BString {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}
impl Ord for BString {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}
impl PartialOrd for BString {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.as_ref().cmp(other.as_ref()))
    }
}

impl BString {
    pub const fn str(s: &'static str) -> Self {
        BString::Static(s)
    }
}

impl From<String> for BString {
    fn from(s: String) -> Self {
        BString::Owned(s)
    }
}
impl From<&String> for BString {
    fn from(s: &String) -> Self {
        BString::Owned(s.to_owned())
    }
}
impl From<&'static str> for BString {
    fn from(s: &'static str) -> Self {
        BString::Static(s)
    }
}
impl From<Arc<String>> for BString {
    fn from(s: Arc<String>) -> Self {
        BString::Arc(s)
    }
}

impl AsRef<str> for BString {
    fn as_ref(&self) -> &str {
        match self {
            BString::Owned(s) => s,
            BString::Static(s) => s,
            BString::Arc(s) => s,
        }
    }
}

impl Borrow<str> for BString {
    fn borrow(&self) -> &str {
        match self {
            BString::Owned(s) => s,
            BString::Static(s) => s,
            BString::Arc(s) => s,
        }
    }
}
impl Deref for BString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            BString::Owned(s) => s,
            BString::Static(s) => s,
            BString::Arc(s) => s,
        }
    }
}

impl Display for BString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BString::Owned(s) => f.write_str(s),
            BString::Static(s) => f.write_str(s),
            BString::Arc(s) => f.write_str(s),
        }
    }
}

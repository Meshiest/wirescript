use std::{borrow::Borrow, fmt::Display, ops::Deref, sync::Arc};

/// A string that can be owned, static, or shared.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BString {
    Owned(String),
    Static(&'static str),
    Arc(Arc<String>),
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

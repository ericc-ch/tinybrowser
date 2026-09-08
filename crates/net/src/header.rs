use std::fmt;

use crate::token::is_token_char;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderError {
    InvalidName(Box<str>),
    InvalidValue(Box<str>),
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(f, "invalid header name: {name:?}"),
            Self::InvalidValue(value) => write!(f, "invalid header value: {value:?}"),
        }
    }
}

impl std::error::Error for HeaderError {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    value: Vec<u8>,
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct HeaderMap {
    entries: Vec<Entry>,
}

impl HeaderMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: &str, value: impl Into<Vec<u8>>) -> Result<(), HeaderError> {
        if name.is_empty() || !name.chars().all(is_token_char) {
            return Err(HeaderError::InvalidName(name.into()));
        }
        let value = value.into();
        if value.iter().any(|&b| (b < 0x20 && b != b'\t') || b == 0x7F) {
            return Err(HeaderError::InvalidValue(
                String::from_utf8_lossy(&value).into_owned().into(),
            ));
        }
        self.entries.push(Entry {
            name: name.to_owned(),
            value,
        });
        Ok(())
    }

    pub fn remove(&mut self, name: &str) {
        self.entries
            .retain(|entry| !entry.name.eq_ignore_ascii_case(name));
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.get_all(name).next()
    }

    pub fn get_all<'s>(&'s self, name: &str) -> impl Iterator<Item = &'s [u8]> {
        self.entries
            .iter()
            .filter(move |entry| entry.name.eq_ignore_ascii_case(name))
            .map(|entry| entry.value.as_slice())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.value.as_slice()))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

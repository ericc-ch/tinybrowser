use std::fmt;

use crate::token::is_token_char;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidMethod(Box<str>);

impl fmt::Display for InvalidMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid HTTP method: {:?}", self.0)
    }
}

impl std::error::Error for InvalidMethod {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Token {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Options,
    Patch,
    Extension(Box<str>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Method(Token);

impl Method {
    pub const GET: Self = Self(Token::Get);
    pub const HEAD: Self = Self(Token::Head);
    pub const POST: Self = Self(Token::Post);
    pub const PUT: Self = Self(Token::Put);
    pub const DELETE: Self = Self(Token::Delete);
    pub const OPTIONS: Self = Self(Token::Options);
    pub const PATCH: Self = Self(Token::Patch);

    // https://fetch.spec.whatwg.org/#methods
    pub fn parse(token: &str) -> Result<Self, InvalidMethod> {
        if token.is_empty() || !token.chars().all(is_token_char) {
            return Err(InvalidMethod(token.into()));
        }
        let uppercased = token.to_ascii_uppercase();
        let stored = match uppercased.as_str() {
            "GET" => Token::Get,
            "HEAD" => Token::Head,
            "POST" => Token::Post,
            "PUT" => Token::Put,
            "DELETE" => Token::Delete,
            "OPTIONS" => Token::Options,
            "PATCH" if uppercased == token => Token::Patch,
            _ => Token::Extension(token.into()),
        };
        Ok(Self(stored))
    }

    #[must_use]
    pub fn is_safe(&self) -> bool {
        matches!(self.0, Token::Get | Token::Head | Token::Options)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        match &self.0 {
            Token::Get => "GET",
            Token::Head => "HEAD",
            Token::Post => "POST",
            Token::Put => "PUT",
            Token::Delete => "DELETE",
            Token::Options => "OPTIONS",
            Token::Patch => "PATCH",
            Token::Extension(ext) => ext,
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

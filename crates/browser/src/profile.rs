//! Named Profile identity. Persistence lives in [`crate::ProfileStore`].

use std::fmt;

/// A named durable browser data set. The implicit name is `default`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    name: ProfileName,
}

impl Profile {
    /// The implicit `default` profile.
    #[must_use]
    pub fn default_named() -> Self {
        Self {
            name: ProfileName::default_named(),
        }
    }

    /// Profile with a parsed name.
    #[must_use]
    pub fn named(name: ProfileName) -> Self {
        Self { name }
    }

    /// Parses `raw` as a profile name. Empty input is `default`.
    ///
    /// # Errors
    ///
    /// [`ProfileError::InvalidName`] when `raw` is not a single path segment.
    pub fn parse(raw: &str) -> Result<Self, ProfileError> {
        Ok(Self {
            name: ProfileName::parse(raw)?,
        })
    }

    /// Name of this profile.
    #[must_use]
    pub fn name(&self) -> &ProfileName {
        &self.name
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self::default_named()
    }
}

/// Parsed profile name. Path separators and `.` / `..` are refused.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProfileName(String);

impl ProfileName {
    /// Parses `raw`. Empty or whitespace-only input becomes `default`.
    ///
    /// # Errors
    ///
    /// [`ProfileError::InvalidName`] when the name cannot be a directory segment.
    pub fn parse(raw: &str) -> Result<Self, ProfileError> {
        let name = raw.trim();
        if name.is_empty() {
            return Ok(Self::default_named());
        }
        if name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name.contains('\0')
        {
            return Err(ProfileError::InvalidName);
        }
        Ok(Self(name.to_owned()))
    }

    /// The implicit profile name.
    #[must_use]
    pub fn default_named() -> Self {
        Self("default".into())
    }

    /// Borrowed name bytes.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a profile name was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// The name is not a single directory segment.
    InvalidName,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => f.write_str("invalid profile name"),
        }
    }
}

impl std::error::Error for ProfileError {}

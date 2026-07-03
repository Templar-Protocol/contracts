use std::fmt;
use std::str::FromStr;

/// A string wrapper that redacts its contents in Debug/Display output.
/// Used for sensitive configuration values like API keys.
#[derive(Clone, serde::Deserialize)]
pub struct RedactedString(String);

impl RedactedString {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for RedactedString {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for RedactedString {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl AsRef<str> for RedactedString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for RedactedString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Debug for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl fmt::Display for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl serde::Serialize for RedactedString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str("***")
    }
}

impl FromStr for RedactedString {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_debug_display_and_serialization() {
        let secret = RedactedString::from("raw-secret-token");

        assert_eq!(format!("{secret:?}"), "***");
        assert_eq!(secret.to_string(), "***");
        assert_eq!(
            serde_json::to_string(&secret).expect("serialization should succeed"),
            "\"***\""
        );
    }

    #[test]
    fn deserializes_raw_secret() {
        let secret = serde_json::from_str::<RedactedString>("\"raw-secret-token\"")
            .expect("deserialization should accept raw secret");

        assert_eq!(secret.as_str(), "raw-secret-token");
    }
}

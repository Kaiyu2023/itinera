use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("currency must be a three-letter uppercase ISO 4217 code")]
pub struct InvalidCurrencyCode;

/// A canonical ISO 4217-shaped currency code.
///
/// The private inner value means code that holds a `CurrencyCode` never needs
/// to remember a separate currency validation call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CurrencyCode(String);

impl CurrencyCode {
    pub fn from_user_input(value: &str) -> Result<Self, InvalidCurrencyCode> {
        Self::try_from(value.trim().to_ascii_uppercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CurrencyCode {
    type Error = InvalidCurrencyCode;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase()) {
            Ok(Self(value))
        } else {
            Err(InvalidCurrencyCode)
        }
    }
}

impl TryFrom<&str> for CurrencyCode {
    type Error = InvalidCurrencyCode;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_string())
    }
}

impl fmt::Display for CurrencyCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CurrencyCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn currency_is_a_validated_transparent_newtype() {
        let normalized = CurrencyCode::from_user_input(" gbp ").expect("valid input");
        assert_eq!(normalized.as_str(), "GBP");
        assert_eq!(serde_json::to_value(&normalized).unwrap(), json!("GBP"));
        assert_eq!(
            serde_json::from_value::<CurrencyCode>(json!("GBP")).unwrap(),
            normalized
        );

        for invalid in ["gbp", "US", "EURO", " U S"] {
            assert!(CurrencyCode::try_from(invalid).is_err());
            assert!(serde_json::from_value::<CurrencyCode>(json!(invalid)).is_err());
        }
    }
}

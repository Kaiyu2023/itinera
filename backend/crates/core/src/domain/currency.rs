use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("currency must contain exactly three ASCII letters")]
pub struct InvalidCurrencyCode;

/// A canonical ISO 4217-shaped currency code.
///
/// The private inner value means code that holds a `CurrencyCode` never needs
/// to remember a separate currency validation call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CurrencyCode(String);

impl CurrencyCode {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CurrencyCode {
    type Err = InvalidCurrencyCode;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let canonical = value.trim().to_ascii_uppercase();
        if canonical.len() == 3 && canonical.bytes().all(|byte| byte.is_ascii_uppercase()) {
            Ok(Self(canonical))
        } else {
            Err(InvalidCurrencyCode)
        }
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
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn currency_is_a_validated_transparent_newtype() {
        let normalized = " gbp ".parse::<CurrencyCode>().expect("valid input");
        assert_eq!(normalized.as_str(), "GBP");
        assert_eq!(serde_json::to_value(&normalized).unwrap(), json!("GBP"));
        assert_eq!(
            serde_json::from_value::<CurrencyCode>(json!("gbp")).unwrap(),
            normalized
        );

        for invalid in ["US", "EURO", " U S", "12$"] {
            assert!(invalid.parse::<CurrencyCode>().is_err());
            assert!(serde_json::from_value::<CurrencyCode>(json!(invalid)).is_err());
        }
    }
}

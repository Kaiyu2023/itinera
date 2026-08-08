#[derive(Debug, Clone, thiserror::Error)]
pub enum InvalidEmail {
    #[error("Invalid email address, only accept patterns like abc@def")]
    InvalidEmailAddressFormats,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Email(String);

pub const MAX_EMAIL_BYTES: usize = 320;

impl Email {
    pub fn parse(raw: &str) -> Result<Self, InvalidEmail> {
        let email = raw.trim().to_ascii_lowercase();
        if email.len() > MAX_EMAIL_BYTES {
            return Err(InvalidEmail::InvalidEmailAddressFormats);
        }
        let (local, domain) = email
            .split_once('@')
            .ok_or(InvalidEmail::InvalidEmailAddressFormats)?;
        if local.is_empty() || domain.is_empty() || domain.contains('@') {
            return Err(InvalidEmail::InvalidEmailAddressFormats);
        }
        Ok(Email(email))
    }

    /// Parses text that is required to already use the canonical spelling.
    pub fn parse_canonical(raw: &str) -> Result<Self, InvalidEmail> {
        let email = Self::parse(raw)?;
        if email.as_str() == raw {
            Ok(email)
        } else {
            Err(InvalidEmail::InvalidEmailAddressFormats)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserId(pub String);

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct User {
    pub id: UserId,
    pub email: Email,
    pub display_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_parse_canonicalizes_padding_and_case() {
        let email = Email::parse("  Cloud.Strife@Proton.ME  ").expect("should parse");
        assert_eq!(email.as_str(), "cloud.strife@proton.me");
    }

    #[test]
    fn email_parse_leaves_canonical_input_unchanged() {
        let canonical = "cloud.strife@proton.me";
        let email = Email::parse(canonical).expect("should parse");
        assert_eq!(email.as_str(), canonical);
    }

    #[test]
    fn canonical_email_parser_rejects_text_that_requires_normalization() {
        assert!(Email::parse_canonical("cloud.strife@proton.me").is_ok());
        assert!(Email::parse_canonical(" Cloud.Strife@Proton.ME ").is_err());
    }

    #[test]
    fn email_parse_maps_case_variants_onto_one_value() {
        // Two spellings of one mailbox must compare equal, or a second login
        // misses the lookup in UserRepo and auto-provisions a duplicate account.
        let lower = Email::parse("cloud.strife@proton.me").expect("should parse");
        let mixed = Email::parse("Cloud.Strife@Proton.me").expect("should parse");
        assert_eq!(lower, mixed);
    }

    #[test]
    fn email_parse_accepts_plus_addressing() {
        let email = Email::parse("cloud.strife+trips@proton.me").expect("should parse");
        assert_eq!(email.as_str(), "cloud.strife+trips@proton.me");
    }

    #[test]
    fn email_parse_enforces_the_storage_boundary() {
        let suffix = "@example.com";
        let at_limit = format!("{}{}", "a".repeat(MAX_EMAIL_BYTES - suffix.len()), suffix);
        let over_limit = format!("a{at_limit}");

        assert_eq!(
            Email::parse(&at_limit).expect("boundary email").as_str(),
            at_limit
        );
        assert!(Email::parse(&over_limit).is_err());
    }

    #[test]
    fn email_parse_should_reject_invalid_emails() {
        for raw in ["", "   ", "abc_123", "abc@", "@def", "abc@def@g"] {
            assert!(
                Email::parse(raw).is_err(),
                "expected {raw:?} to be rejected"
            );
        }
    }
}

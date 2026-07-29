#[derive(Debug, Clone, thiserror::Error)]
pub enum InvalidEmail {
    #[error("Invalid email address, only accept patterns like abc@def")]
    InvalidEmailAddressFormats,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Email(String);

impl Email {
    pub fn parse(raw: &str) -> Result<Self, InvalidEmail> {
        let email = raw.trim().to_ascii_lowercase();
        let (local, domain) = email
            .split_once('@')
            .ok_or(InvalidEmail::InvalidEmailAddressFormats)?;
        if local.is_empty() || domain.is_empty() || domain.contains('@') {
            return Err(InvalidEmail::InvalidEmailAddressFormats);
        }
        Ok(Email(email))
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
    fn email_parse_should_reject_invalid_emails() {
        for raw in ["", "   ", "abc_123", "abc@", "@def", "abc@def@g"] {
            assert!(
                Email::parse(raw).is_err(),
                "expected {raw:?} to be rejected"
            );
        }
    }
}

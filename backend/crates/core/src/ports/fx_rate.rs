use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FxRateError {
    #[error("the currency pair is not supported")]
    UnsupportedCurrency,
    #[error("the exchange-rate provider is unavailable")]
    Unavailable,
    #[error("the exchange-rate provider returned an invalid response")]
    InvalidResponse,
}

#[async_trait]
pub trait FxRateProvider: Send + Sync {
    /// Returns the multiplier that converts one unit of `currency` into
    /// `base_currency`.
    async fn rate_to_base(&self, currency: &str, base_currency: &str) -> Result<f64, FxRateError>;
}

pub mod trip_repo;

use async_trait::async_trait;
use itinera_core::ports::fx_rate::{FxRateError, FxRateProvider};

pub struct FixedFxRateProvider(pub f64);

#[async_trait]
impl FxRateProvider for FixedFxRateProvider {
    async fn rate_to_base(&self, currency: &str, base_currency: &str) -> Result<f64, FxRateError> {
        if currency == base_currency {
            Ok(1.0)
        } else {
            Ok(self.0)
        }
    }
}

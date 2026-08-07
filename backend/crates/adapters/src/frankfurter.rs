use std::time::Duration;

use async_trait::async_trait;
use itinera_core::ports::fx_rate::{FxRateError, FxRateProvider};
use reqwest::{Client, StatusCode, redirect::Policy};
use serde::Deserialize;

const API_ORIGIN: &str = "https://api.frankfurter.dev";
const MAX_RESPONSE_BYTES: usize = 16 * 1_024;

#[derive(Clone)]
pub struct FrankfurterFxRateProvider {
    client: Client,
    api_origin: String,
}

impl FrankfurterFxRateProvider {
    pub fn new() -> Result<Self, FrankfurterBuildError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .redirect(Policy::none())
            .user_agent("itinera/1.0")
            .build()?;
        Ok(Self {
            client,
            api_origin: API_ORIGIN.to_string(),
        })
    }

    #[cfg(test)]
    fn with_test_origin(api_origin: String) -> Result<Self, FrankfurterBuildError> {
        let mut provider = Self::new()?;
        provider.api_origin = api_origin;
        Ok(provider)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrankfurterBuildError {
    #[error("could not build the exchange-rate HTTP client")]
    HttpClient(#[from] reqwest::Error),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RateResponse {
    date: String,
    base: String,
    quote: String,
    rate: f64,
}

#[async_trait]
impl FxRateProvider for FrankfurterFxRateProvider {
    async fn rate_to_base(&self, currency: &str, base_currency: &str) -> Result<f64, FxRateError> {
        if !valid_currency(currency) || !valid_currency(base_currency) {
            return Err(FxRateError::UnsupportedCurrency);
        }
        if currency == base_currency {
            return Ok(1.0);
        }
        let url = format!(
            "{}/v2/rate/{currency}/{base_currency}",
            self.api_origin.trim_end_matches('/')
        );
        let mut response = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| FxRateError::Unavailable)?;
        if matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY
        ) {
            return Err(FxRateError::UnsupportedCurrency);
        }
        if !response.status().is_success() {
            return Err(FxRateError::Unavailable);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(FxRateError::InvalidResponse);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !content_type.starts_with("application/json") {
            return Err(FxRateError::InvalidResponse);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| FxRateError::Unavailable)?
        {
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
                return Err(FxRateError::InvalidResponse);
            }
            body.extend_from_slice(&chunk);
        }
        decode_rate_at(
            &body,
            currency,
            base_currency,
            chrono::Utc::now().date_naive(),
        )
    }
}

fn decode_rate_at(
    body: &[u8],
    expected_base: &str,
    expected_quote: &str,
    today: chrono::NaiveDate,
) -> Result<f64, FxRateError> {
    let response: RateResponse =
        serde_json::from_slice(body).map_err(|_| FxRateError::InvalidResponse)?;
    let date = chrono::NaiveDate::parse_from_str(&response.date, "%Y-%m-%d")
        .map_err(|_| FxRateError::InvalidResponse)?;
    if response.base != expected_base
        || response.quote != expected_quote
        || date.format("%Y-%m-%d").to_string() != response.date
        || date > today
        || today.signed_duration_since(date).num_days() > 7
        || !response.rate.is_finite()
        || response.rate <= 0.0
        || response.rate > 1_000_000.0
    {
        return Err(FxRateError::InvalidResponse);
    }
    Ok(response.rate)
}

fn valid_currency(value: &str) -> bool {
    value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::{Duration, timeout},
    };

    use super::*;

    async fn test_server(response: Vec<u8>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test address");
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok(Ok((mut stream, _))) =
                    timeout(Duration::from_millis(250), listener.accept()).await
                else {
                    break;
                };
                let mut request = vec![0; 8 * 1024];
                let read = stream.read(&mut request).await.expect("read request");
                let request = String::from_utf8_lossy(&request[..read]);
                assert!(request.starts_with("GET /v2/rate/JPY/GBP HTTP/1.1\r\n"));
                seen.fetch_add(1, Ordering::SeqCst);
                stream.write_all(&response).await.expect("write response");
                stream.shutdown().await.expect("close response");
            }
        });
        (format!("http://{address}"), requests)
    }

    fn response(status: &str, content_type: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn current_body() -> String {
        format!(
            r#"{{"date":"{}","base":"JPY","quote":"GBP","rate":0.005}}"#,
            chrono::Utc::now().date_naive()
        )
    }

    #[test]
    fn rate_response_is_pair_bound_bounded_and_strict() {
        let body = br#"{"date":"2026-08-06","base":"JPY","quote":"GBP","rate":0.005}"#;
        let today = chrono::NaiveDate::from_ymd_opt(2026, 8, 6).expect("valid date");
        assert_eq!(decode_rate_at(body, "JPY", "GBP", today), Ok(0.005));
        assert_eq!(
            decode_rate_at(body, "EUR", "GBP", today),
            Err(FxRateError::InvalidResponse)
        );
        assert_eq!(
            decode_rate_at(
                br#"{"date":"2026-08-06","base":"JPY","quote":"GBP","rate":0.005,"forged":true}"#,
                "JPY",
                "GBP",
                today
            ),
            Err(FxRateError::InvalidResponse)
        );
        assert_eq!(
            decode_rate_at(
                br#"{"date":"2026-08-06","base":"JPY","quote":"GBP","rate":0}"#,
                "JPY",
                "GBP",
                today
            ),
            Err(FxRateError::InvalidResponse)
        );
        assert_eq!(
            decode_rate_at(
                br#"{"date":"2026-07-01","base":"JPY","quote":"GBP","rate":0.005}"#,
                "JPY",
                "GBP",
                today
            ),
            Err(FxRateError::InvalidResponse)
        );
    }

    #[tokio::test]
    async fn fixed_path_json_content_type_and_status_mapping_are_enforced() {
        let body = current_body();
        let (origin, requests) = test_server(response("200 OK", "application/json", &body)).await;
        let provider = FrankfurterFxRateProvider::with_test_origin(origin).expect("provider");
        assert_eq!(provider.rate_to_base("JPY", "GBP").await, Ok(0.005));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(requests.load(Ordering::SeqCst), 1);

        for (status, expected) in [
            ("400 Bad Request", FxRateError::UnsupportedCurrency),
            ("404 Not Found", FxRateError::UnsupportedCurrency),
            ("422 Unprocessable Entity", FxRateError::UnsupportedCurrency),
            ("500 Internal Server Error", FxRateError::Unavailable),
        ] {
            let (origin, _) = test_server(response(status, "application/json", "{}")).await;
            let provider = FrankfurterFxRateProvider::with_test_origin(origin).expect("provider");
            assert_eq!(provider.rate_to_base("JPY", "GBP").await, Err(expected));
        }

        let (origin, _) = test_server(response("200 OK", "text/plain", &body)).await;
        let provider = FrankfurterFxRateProvider::with_test_origin(origin).expect("provider");
        assert_eq!(
            provider.rate_to_base("JPY", "GBP").await,
            Err(FxRateError::InvalidResponse)
        );
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let redirect = b"HTTP/1.1 302 Found\r\nLocation: /v2/rate/JPY/GBP\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
        let (origin, requests) = test_server(redirect).await;
        let provider = FrankfurterFxRateProvider::with_test_origin(origin).expect("provider");
        assert_eq!(
            provider.rate_to_base("JPY", "GBP").await,
            Err(FxRateError::Unavailable)
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(requests.load(Ordering::SeqCst), 1, "redirect was followed");
    }

    #[tokio::test]
    async fn chunked_responses_are_bounded_without_a_content_length() {
        let oversized = "x".repeat(MAX_RESPONSE_BYTES + 1);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n{}\r\n0\r\n\r\n",
            oversized.len(),
            oversized
        )
        .into_bytes();
        let (origin, _) = test_server(response).await;
        let provider = FrankfurterFxRateProvider::with_test_origin(origin).expect("provider");
        assert_eq!(
            provider.rate_to_base("JPY", "GBP").await,
            Err(FxRateError::InvalidResponse)
        );
    }
}

use chrono::{SecondsFormat, Utc};
use itinera_core::ports::clock::Clock;

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> String {
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
    }
}

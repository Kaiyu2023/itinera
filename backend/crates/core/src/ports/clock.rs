pub trait Clock: Send + Sync {
    /// Current UTC instant, formatted as RFC 3339 for the public contract.
    fn now(&self) -> String;
}

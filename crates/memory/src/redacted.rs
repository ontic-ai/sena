use std::fmt;

/// A wrapper that prevents sensitive content from appearing in logs.
///
/// `Redacted<T>` holds a value of type `T` but implements `Debug` and
/// `Display` to output `[REDACTED]` instead of the actual content.
/// This ensures sensitive memory node content never reaches the log sink.
///
/// Access the inner value via `.inner()` for legitimate processing.
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Wrap a value to prevent it from appearing in logs.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Access the inner value for legitimate processing.
    #[allow(dead_code)]
    pub(crate) fn inner(&self) -> &T {
        &self.0
    }

    /// Consume the wrapper and return the inner value.
    #[allow(dead_code)]
    pub(crate) fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_is_redacted() {
        let secret = Redacted::new("sensitive memory content".to_string());
        let debug_output = format!("{:?}", secret);
        assert_eq!(debug_output, "[REDACTED]");
        assert!(!debug_output.contains("sensitive"));
    }

    #[test]
    fn display_output_is_redacted() {
        let secret = Redacted::new("memory node data".to_string());
        let display_output = format!("{}", secret);
        assert_eq!(display_output, "[REDACTED]");
        assert!(!display_output.contains("memory"));
    }

    #[test]
    fn inner_access_returns_original_value() {
        let secret = Redacted::new(vec![1u8, 2, 3]);
        assert_eq!(secret.inner(), &vec![1u8, 2, 3]);
    }

    #[test]
    fn into_inner_consumes_wrapper() {
        let secret = Redacted::new("graph data".to_string());
        let value = secret.into_inner();
        assert_eq!(value, "graph data");
    }

    #[test]
    fn redacted_in_format_string_stays_redacted() {
        let content = Redacted::new("user's personal knowledge");
        let log_line = format!("Memory query result: {}", content);
        assert_eq!(log_line, "Memory query result: [REDACTED]");
        assert!(!log_line.contains("personal"));
    }
}

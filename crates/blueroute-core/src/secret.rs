use std::fmt;

const REDACTED: &str = "[REDACTED]";

/// Explicit wrapper for values that must not be exposed through ordinary formatting.
///
/// Secret contents are only available through [`Secret::expose`], making disclosure an
/// intentional call-site decision. Both `Debug` and `Display` formatting are redacted so
/// accidental structured logging or diagnostic formatting does not reveal the value.
pub struct Secret<T> {
    value: T,
}

impl<T> Secret<T> {
    pub const fn new(value: T) -> Self {
        Self { value }
    }

    /// Explicitly borrows the wrapped secret value.
    pub const fn expose(&self) -> &T {
        &self.value
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Secret").field(&REDACTED).finish()
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_redact_secret_contents() {
        let secret = Secret::new("correct horse battery staple".to_owned());

        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert_eq!(debug, "Secret(\"[REDACTED]\")");
        assert_eq!(display, REDACTED);
        assert!(!debug.contains("correct horse battery staple"));
        assert!(!display.contains("correct horse battery staple"));
    }

    #[test]
    fn secret_contents_require_explicit_exposure() {
        let secret = Secret::new(vec![1_u8, 2, 3, 4]);
        assert_eq!(secret.expose(), &[1, 2, 3, 4]);
    }
}

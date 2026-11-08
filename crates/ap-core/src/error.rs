//! Error type shared by all backends.

use std::time::Duration;

/// Result alias using [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors from devices, providers and configuration.
///
/// [`Error::Setup`] is for problems the user can fix (missing driver, pairing,
/// Developer Mode, ...). Its `hint` must say what to do, in plain words.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The backend cannot do this operation.
    #[error("not supported by this device/backend: {0}")]
    Unsupported(String),

    /// No device is connected, or the connection was lost.
    #[error("device not connected: {0}")]
    NotConnected(String),

    /// No device with this id.
    #[error("device not found: {0}")]
    DeviceNotFound(String),

    /// The user must change something on the PC or the phone.
    #[error("{problem}\n  fix: {hint}")]
    Setup { problem: String, hint: String },

    /// An argument is invalid (e.g. point outside 0..1).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// The action was refused by a safety rule.
    #[error("refused by safety policy: {0}")]
    Refused(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The device or a service answered something unexpected.
    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("timed out after {0:?}")]
    Timeout(Duration),

    #[error("config error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Build a [`Error::Setup`] error with a fix hint.
    pub fn setup(problem: impl Into<String>, hint: impl Into<String>) -> Self {
        Error::Setup {
            problem: problem.into(),
            hint: hint.into(),
        }
    }

    pub fn unsupported(what: impl Into<String>) -> Self {
        Error::Unsupported(what.into())
    }

    pub fn protocol(msg: impl std::fmt::Display) -> Self {
        Error::Protocol(msg.to_string())
    }

    /// The fix hint, if this is a setup error.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Error::Setup { hint, .. } => Some(hint),
            _ => None,
        }
    }

    pub fn is_unsupported(&self) -> bool {
        matches!(self, Error::Unsupported(_))
    }
}

impl From<toml::de::Error> for Error {
    fn from(e: toml::de::Error) -> Self {
        Error::Config(e.to_string())
    }
}

impl From<toml::ser::Error> for Error {
    fn from(e: toml::ser::Error) -> Self {
        Error::Config(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_shows_hint() {
        let e = Error::setup(
            "usbmuxd not running",
            "Install Apple Devices from the Microsoft Store",
        );
        assert_eq!(
            e.hint(),
            Some("Install Apple Devices from the Microsoft Store")
        );
        assert!(e.to_string().contains("fix: Install Apple Devices"));
        assert!(Error::unsupported("ui_tree").is_unsupported());
    }
}

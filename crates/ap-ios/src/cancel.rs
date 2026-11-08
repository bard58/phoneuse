//! Cancel an in-flight iOS action from another task or thread.
//!
//! [`CancelHandle::cancel`] stops every device call that was submitted before
//! the cancel: a queued call does not start, and a running long action (text,
//! a drag, a long press, a pinch, a button sequence) stops at the next stroke
//! or point and releases what it holds. Calls submitted after the cancel run
//! normally. The stopped call returns the error from [`cancelled`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ap_core::Error;

/// Text at the start of the error that a cancelled call returns.
pub const CANCELLED_PREFIX: &str = "cancelled";

/// The error a cancelled call returns. It is not a transport error, so no
/// layer retries it.
pub fn cancelled() -> Error {
    Error::Other(format!(
        "{CANCELLED_PREFIX}: the action was stopped before it finished; take a screenshot to see the state"
    ))
}

/// True when `e` is the error from [`cancelled`].
pub fn is_cancelled(e: &Error) -> bool {
    matches!(e, Error::Other(m) if m.starts_with(CANCELLED_PREFIX))
}

/// Cloneable, `Send + Sync` handle that cancels the device's in-flight calls.
/// Get it from [`IosDevice::cancel_handle`](crate::IosDevice::cancel_handle).
#[derive(Debug, Clone, Default)]
pub struct CancelHandle {
    generation: Arc<AtomicU64>,
}

impl CancelHandle {
    /// Stop every call submitted before now. Returns at once. It does not
    /// wait for the running call to stop.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// A check bound to the calls submitted from now on.
    pub(crate) fn check(&self) -> CancelCheck {
        CancelCheck {
            generation: self.generation.clone(),
            at: self.generation.load(Ordering::SeqCst),
        }
    }
}

/// Tells one call whether it was cancelled.
#[derive(Debug, Clone, Default)]
pub struct CancelCheck {
    generation: Arc<AtomicU64>,
    at: u64,
}

impl CancelCheck {
    /// True after [`CancelHandle::cancel`] was called for this call.
    pub fn is_set(&self) -> bool {
        self.generation.load(Ordering::SeqCst) != self.at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_hits_only_earlier_calls() {
        let h = CancelHandle::default();
        let before = h.check();
        assert!(!before.is_set());
        h.cancel();
        assert!(before.is_set());
        let after = h.check();
        assert!(!after.is_set());
        assert!(is_cancelled(&cancelled()));
        assert!(!is_cancelled(&Error::Other("x".into())));
        assert!(!crate::errors::is_transport(&cancelled()));
    }
}

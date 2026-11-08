//! Map idevice errors to ap-core errors with plain fix hints.

use ap_core::Error;
use idevice::IdeviceError;

pub(crate) const DDI_HINT: &str = "mount the Developer Disk Image: unlock the iPhone and run `agentphone doctor` (it downloads and mounts the image)";
pub(crate) const DEVMODE_HINT: &str = "turn on Developer Mode: run `agentphone doctor`, then on the iPhone open Settings > Privacy & Security > Developer Mode, switch it on, restart, and confirm";
pub(crate) const PAIR_HINT: &str = "unlock the iPhone, connect it with a cable, and tap Trust (or run `agentphone doctor` to pair again)";

/// Convert an idevice error. `ctx` names the step ("tap", "RSD handshake").
pub(crate) fn map(ctx: &str, e: IdeviceError) -> Error {
    match e {
        IdeviceError::Socket(io) => Error::NotConnected(format!("{ctx}: {io}")),
        IdeviceError::NoEstablishedConnection | IdeviceError::SessionInactive => {
            Error::NotConnected(format!("{ctx}: connection closed"))
        }
        IdeviceError::DeviceLocked => Error::setup(
            format!("{ctx}: the iPhone is locked"),
            "unlock the iPhone and try again",
        ),
        IdeviceError::PasswordProtected => Error::setup(
            format!("{ctx}: the iPhone is locked with a passcode"),
            "unlock the iPhone, then tap Trust when it asks",
        ),
        IdeviceError::PairingDialogResponsePending => Error::setup(
            format!("{ctx}: the iPhone shows the Trust dialog"),
            "tap Trust on the iPhone and enter the passcode, then try again",
        ),
        IdeviceError::UserDeniedPairing => Error::setup(
            format!("{ctx}: Trust was denied on the iPhone"),
            "reconnect the cable and tap Trust (Settings > General > Transfer or Reset > Reset Location & Privacy resets the choice)",
        ),
        IdeviceError::InvalidHostID => Error::setup(
            format!("{ctx}: the iPhone does not know this computer's pairing"),
            PAIR_HINT,
        ),
        IdeviceError::DeveloperModeNotEnabled => {
            Error::setup(format!("{ctx}: Developer Mode is off"), DEVMODE_HINT)
        }
        IdeviceError::ImageNotMounted => Error::setup(
            format!("{ctx}: the Developer Disk Image is not mounted"),
            DDI_HINT,
        ),
        IdeviceError::ServiceNotFound => Error::setup(
            format!("{ctx}: the iPhone does not offer this service"),
            DDI_HINT,
        ),
        IdeviceError::DeviceNotFound => Error::DeviceNotFound(ctx.to_string()),
        other => Error::Protocol(format!("{ctx}: {other}")),
    }
}

/// A closure for `map_err`.
pub(crate) fn ctx(ctx: &str) -> impl Fn(IdeviceError) -> Error + '_ {
    move |e| map(ctx, e)
}

/// True when the error means the link is gone and a reconnect can help.
pub(crate) fn is_transport(e: &Error) -> bool {
    matches!(e, Error::NotConnected(_) | Error::Io(_) | Error::Timeout(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping() {
        let e = map(
            "tap",
            IdeviceError::Socket(std::io::Error::other("broken pipe")),
        );
        assert!(is_transport(&e));
        let e = map("mount", IdeviceError::DeveloperModeNotEnabled);
        assert!(e.hint().unwrap().contains("Developer Mode"));
        assert!(!is_transport(&map("x", IdeviceError::DeviceLocked)));
    }
}

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

/// The CoreDevice error code for "Remote control requires iOS 27.0 or later
/// on this device" (go-ios #843 and #849, quern #624).
pub(crate) const STREAM_REFUSED_CODE: &str = "9021";

/// True when the text of a device reply refuses the screen stream: it holds
/// the code 9021 as a whole number, or the "requires iOS 27" text.
pub(crate) fn is_stream_refusal_text(s: &str) -> bool {
    if s.contains("Remote control requires iOS") || s.contains("requires iOS 27") {
        return true;
    }
    let b = s.as_bytes();
    s.match_indices(STREAM_REFUSED_CODE).any(|(i, m)| {
        let before = i.checked_sub(1).and_then(|j| b.get(j));
        let after = b.get(i + m.len());
        !before.is_some_and(u8::is_ascii_digit) && !after.is_some_and(u8::is_ascii_digit)
    })
}

/// True when `e` is the device refusing the displayservice stream.
///
/// idevice 0.1.68 gives a refused `startmediastream` as
/// `CoreDeviceError::DeviceError` with the `CoreDevice.error` value in Debug
/// form (`core_device/mod.rs` `invoke_inner`). [`map`] turns it into
/// [`Error::Protocol`] with that text. A transport error is never a refusal.
pub(crate) fn is_stream_refusal(e: &Error) -> bool {
    match e {
        Error::Protocol(m) | Error::Other(m) => is_stream_refusal_text(m),
        Error::Setup { problem, .. } => is_stream_refusal_text(problem),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_refusal_is_detected() {
        // The shape idevice gives: DeviceError(format!("{:?}", plist value)).
        let reply = r#"Dictionary({"code": Integer(9021), "domain": String("com.apple.dt.CoreDeviceError"), "userInfo": Dictionary({"NSLocalizedDescription": String("Remote control requires iOS 27.0 or later on this device.")})})"#;
        let e = map(
            "start audio stream",
            IdeviceError::CoreDevice(idevice::core_device::CoreDeviceError::DeviceError(
                reply.into(),
            )),
        );
        assert!(matches!(e, Error::Protocol(_)), "{e}");
        assert!(is_stream_refusal(&e), "{e}");
        // Only the code, no text.
        assert!(is_stream_refusal_text(
            "Dictionary({\"code\": Integer(9021)})"
        ));
        assert!(is_stream_refusal_text("error 9021"));
        // Other numbers that hold 9021 are not the code.
        assert!(!is_stream_refusal_text("port 59021"));
        assert!(!is_stream_refusal_text("id 90210"));
        assert!(!is_stream_refusal(&Error::Protocol(
            "start video stream: bad offer".into()
        )));
        // A lost link is a transport error, not a refusal.
        assert!(!is_stream_refusal(&Error::NotConnected("9021".into())));
    }

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

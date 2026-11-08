//! The backend trait inside ap-ios.
//!
//! An [`IosDevice`](crate::IosDevice) owns one boxed [`IosBackend`] and runs
//! it on its own worker thread (see `worker.rs`), so backend futures do not
//! need to be `Send` and can use the large idevice futures freely.
//!
//! Where a new backend plugs in (for example WebDriverAgent later):
//! 1. Add a variant to [`crate::version::BackendKind`] and map its config
//!    name in [`crate::version::select_backends`].
//! 2. Implement [`IosBackend`] for it.
//! 3. Build it in `device::open_backend` (match on the kind).
//!
//! The public API (`IosProvider`, `IosDevice`, `setup`) does not change.

use std::time::Duration;

use ap_core::{Button, Capabilities, Error, Frame, Key, Point, Result};
use async_trait::async_trait;

/// One way to drive an iPhone. All points are normalized (0..1).
#[async_trait(?Send)]
pub trait IosBackend {
    /// Short name for `DeviceInfo::backend` ("hid").
    fn name(&self) -> &'static str;

    fn capabilities(&self) -> Capabilities;

    async fn screenshot(&mut self) -> Result<Frame>;

    async fn tap(&mut self, at: Point) -> Result<()>;

    async fn double_tap(&mut self, at: Point) -> Result<()>;

    async fn long_press(&mut self, at: Point, duration: Duration) -> Result<()>;

    async fn swipe(&mut self, from: Point, to: Point, duration: Duration) -> Result<()>;

    /// Two-finger pinch around `center`. `from`/`to` are finger distances
    /// (normalized). `from < to` zooms in.
    async fn pinch(
        &mut self,
        _center: Point,
        _from: f32,
        _to: f32,
        _duration: Duration,
    ) -> Result<()> {
        Err(Error::unsupported("pinch"))
    }

    async fn type_text(&mut self, text: &str) -> Result<()>;

    async fn key(&mut self, key: &Key) -> Result<()>;

    async fn button(&mut self, button: Button) -> Result<()>;

    async fn launch_app(&mut self, bundle_id: &str) -> Result<()>;

    async fn list_apps(&mut self) -> Result<Vec<String>> {
        Err(Error::unsupported("list_apps"))
    }

    async fn ui_tree(&mut self) -> Result<Option<String>> {
        Err(Error::unsupported("ui_tree"))
    }

    /// Close sessions. Called when the device is dropped.
    async fn shutdown(&mut self) {}
}

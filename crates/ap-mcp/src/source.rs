//! Where the server gets its phone from.
//!
//! The server never talks to a backend crate directly. It asks a
//! [`DeviceSource`] for a device on the first tool call, keeps it, and asks
//! again after the connection is lost or after `end_session`.

use std::sync::Arc;

use ap_core::{Device, DeviceInfo, DeviceProvider, Error, Result, async_trait};

/// Connects a phone on demand.
#[async_trait]
pub trait DeviceSource: Send + Sync + 'static {
    /// Connect (or reconnect) a device. Called lazily on the first tool call,
    /// and again after the server dropped the device (lost connection,
    /// `end_session`). Problems the user can fix should be [`Error::Setup`]
    /// with a hint: the hint reaches the MCP client as text.
    async fn connect(&self) -> Result<Arc<dyn Device>>;

    /// Short human description for `status` ("iOS + Android providers").
    fn describe(&self) -> String {
        "device source".into()
    }
}

/// A source that always returns the same, already connected device.
#[derive(Clone)]
pub struct StaticSource(pub Arc<dyn Device>);

impl StaticSource {
    pub fn new(device: impl Device + 'static) -> Self {
        Self(Arc::new(device))
    }
}

#[async_trait]
impl DeviceSource for StaticSource {
    async fn connect(&self) -> Result<Arc<dyn Device>> {
        Ok(self.0.clone())
    }

    fn describe(&self) -> String {
        format!("fixed device {}", self.0.info().id)
    }
}

/// A source over [`DeviceProvider`]s (iOS, Android, ...): discovers devices
/// and connects the one with `device_id`, or the only one found.
#[derive(Clone)]
pub struct ProviderSource {
    providers: Vec<Arc<dyn DeviceProvider>>,
    device_id: Option<String>,
}

impl ProviderSource {
    /// `device_id`: a UDID / serial, or `None` for "the only connected device".
    pub fn new(providers: Vec<Arc<dyn DeviceProvider>>, device_id: Option<String>) -> Self {
        Self {
            providers,
            device_id,
        }
    }
}

#[async_trait]
impl DeviceSource for ProviderSource {
    async fn connect(&self) -> Result<Arc<dyn Device>> {
        if self.providers.is_empty() {
            return Err(Error::setup(
                "no device providers are configured",
                "build agentphone with iOS or Android support",
            ));
        }
        let mut found: Vec<(usize, DeviceInfo)> = Vec::new();
        let mut errors: Vec<Error> = Vec::new();
        for (i, p) in self.providers.iter().enumerate() {
            match p.discover().await {
                Ok(list) => found.extend(list.into_iter().map(|d| (i, d))),
                Err(e) => {
                    tracing::debug!(provider = p.name(), error = %e, "discover failed");
                    errors.push(e);
                }
            }
        }
        let pick = match &self.device_id {
            Some(id) => found.iter().find(|(_, d)| &d.id == id).cloned(),
            None if found.len() == 1 => found.first().cloned(),
            None => None,
        };
        if let Some((i, info)) = pick {
            let dev = self.providers[i].connect(&info.id).await?;
            return Ok(Arc::from(dev));
        }
        // Not listed: let each provider resolve the id itself, as the CLI does
        // (Android connects a Wi-Fi host:port, iOS accepts a UDID prefix).
        if let Some(id) = &self.device_id {
            let mut useful: Option<Error> = None;
            for p in &self.providers {
                match p.connect(id).await {
                    Ok(dev) => return Ok(Arc::from(dev)),
                    Err(Error::DeviceNotFound(_) | Error::Unsupported(_)) => {}
                    Err(e) => {
                        tracing::debug!(provider = p.name(), error = %e, "connect by id failed");
                        useful.get_or_insert(e);
                    }
                }
            }
            if let Some(e) = useful {
                return Err(e);
            }
        }
        if found.len() > 1 && self.device_id.is_none() {
            let ids: Vec<String> = found
                .iter()
                .map(|(_, d)| format!("{} ({})", d.id, d.name))
                .collect();
            return Err(Error::setup(
                format!("{} phones are connected", found.len()),
                format!(
                    "choose one: start the server with --device <id>, or set default_device in the config. Found: {}",
                    ids.join(", ")
                ),
            ));
        }
        // Nothing usable: a setup error from discovery says best what to fix.
        if let Some(pos) = errors.iter().position(|e| e.hint().is_some()) {
            return Err(errors.swap_remove(pos));
        }
        if let Some(e) = errors.into_iter().next() {
            return Err(e);
        }
        match &self.device_id {
            Some(id) => Err(Error::DeviceNotFound(format!(
                "{id} (run `agentphone devices` to list connected phones)"
            ))),
            None => Err(Error::setup(
                "no phone found",
                "connect the phone with a USB cable, unlock it and accept \"Trust This Computer\" (iPhone) or \"Allow USB debugging\" (Android), then run `agentphone doctor`",
            )),
        }
    }

    fn describe(&self) -> String {
        let names: Vec<&str> = self.providers.iter().map(|p| p.name()).collect();
        match &self.device_id {
            Some(id) => format!("providers [{}], device {id}", names.join(", ")),
            None => format!("providers [{}], first device found", names.join(", ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ap_core::mock::MockDevice;

    /// Lists nothing, but connects one id that it can resolve itself
    /// (like Android `adb connect host:port`).
    struct Unlisted {
        resolves: &'static str,
    }

    #[async_trait]
    impl DeviceProvider for Unlisted {
        fn name(&self) -> &str {
            "unlisted"
        }
        async fn discover(&self) -> Result<Vec<DeviceInfo>> {
            Ok(Vec::new())
        }
        async fn connect(&self, id: &str) -> Result<Box<dyn Device>> {
            if id == self.resolves {
                Ok(Box::new(MockDevice::new()))
            } else {
                Err(Error::DeviceNotFound(id.into()))
            }
        }
    }

    fn source(id: &str) -> ProviderSource {
        ProviderSource::new(
            vec![
                Arc::new(Unlisted { resolves: "other" }),
                Arc::new(Unlisted {
                    resolves: "1.2.3.4:5555",
                }),
            ],
            Some(id.into()),
        )
    }

    #[tokio::test]
    async fn unlisted_id_falls_back_to_provider_connect() {
        assert!(source("1.2.3.4:5555").connect().await.is_ok());
        let e = source("nope").connect().await.err().expect("not found");
        assert!(matches!(e, Error::DeviceNotFound(_)), "{e}");
    }
}

/// A source over a shared [`ap_core::mock::MockDevice`] (clones share its call log).
#[cfg(feature = "mock")]
#[derive(Clone, Default)]
pub struct MockSource(pub ap_core::mock::MockDevice);

#[cfg(feature = "mock")]
#[async_trait]
impl DeviceSource for MockSource {
    async fn connect(&self) -> Result<Arc<dyn Device>> {
        Ok(Arc::new(self.0.clone()))
    }

    fn describe(&self) -> String {
        "mock device".into()
    }
}

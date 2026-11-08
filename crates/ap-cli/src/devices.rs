//! Finding and connecting phones across iOS and Android.

use anyhow::{Result, bail};
use ap_core::{Config, Device, DeviceInfo, DeviceProvider, Error};
use serde::Serialize;

use crate::ios;

/// All providers, iPhone first.
pub fn providers(cfg: &Config) -> Vec<Box<dyn DeviceProvider>> {
    vec![
        ios::provider(cfg),
        Box::new(ap_android::AndroidProvider::new(cfg.android.clone())),
    ]
}

/// Result of asking every provider.
#[derive(Debug, Default, Serialize)]
pub struct Discovery {
    pub devices: Vec<DeviceInfo>,
    /// Per-provider problems: (provider, problem, fix).
    pub problems: Vec<Problem>,
}

#[derive(Debug, Serialize)]
pub struct Problem {
    pub provider: String,
    pub problem: String,
    pub fix: Option<String>,
}

impl Problem {
    fn new(provider: &str, e: Error) -> Self {
        let (problem, fix) = crate::ui::explain(&anyhow::Error::new(e));
        Self {
            provider: provider.to_string(),
            problem,
            fix,
        }
    }
}

/// Ask all providers in parallel.
pub async fn discover(providers: &[Box<dyn DeviceProvider>]) -> Discovery {
    let results = futures::future::join_all(
        providers
            .iter()
            .map(|p| async move { (p.name().to_string(), p.discover().await) }),
    )
    .await;
    let mut d = Discovery::default();
    for (name, r) in results {
        match r {
            Ok(list) => d.devices.extend(list),
            Err(e) => d.problems.push(Problem::new(&name, e)),
        }
    }
    d
}

/// Pick the device id: `--device`, then the config default, then the only phone.
pub fn choose(wanted: Option<&str>, cfg: &Config, found: &Discovery) -> Result<String> {
    if let Some(id) = wanted.or(cfg.default_device.as_deref()) {
        return Ok(id.to_string());
    }
    match found.devices.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => {
            let mut msg = String::from("no phone found");
            for p in &found.problems {
                msg.push_str(&format!("\n  {}: {}", p.provider, p.problem));
                if let Some(f) = &p.fix {
                    msg.push_str(&format!("\n    fix: {f}"));
                }
            }
            Err(Error::setup(
                msg,
                "plug in the phone and unlock it, then run `agentphone doctor` (first time: `agentphone setup`)",
            )
            .into())
        }
        many => {
            let list = many
                .iter()
                .map(|d| format!("{} ({}, {})", d.id, d.name, d.platform))
                .collect::<Vec<_>>()
                .join(", ");
            Err(Error::setup(
                format!("more than one phone is connected: {list}"),
                "choose one with `agentphone use <id>` or pass `--device <id>`",
            )
            .into())
        }
    }
}

/// Connect to the chosen phone. Tries the provider that listed it first.
pub async fn connect(cfg: &Config, wanted: Option<&str>) -> Result<Box<dyn Device>> {
    let providers = providers(cfg);
    let found = discover(&providers).await;
    let id = choose(wanted, cfg, &found)?;
    if let Some(info) = found.devices.iter().find(|d| d.id == id) {
        let name = match info.platform {
            ap_core::Platform::Ios => "ios",
            ap_core::Platform::Android => "android",
        };
        if let Some(p) = providers.iter().find(|p| p.name() == name) {
            return Ok(p.connect(&id).await?);
        }
    }
    // Not listed (for example an Android Wi-Fi address): try each provider.
    let mut useful = None;
    for p in &providers {
        match p.connect(&id).await {
            Ok(d) => return Ok(d),
            Err(Error::DeviceNotFound(_) | Error::Unsupported(_)) => {}
            Err(e) => {
                useful.get_or_insert(e);
            }
        }
    }
    match useful {
        Some(e) => Err(e.into()),
        None => bail!(Error::DeviceNotFound(id)),
    }
}

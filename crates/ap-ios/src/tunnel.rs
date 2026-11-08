//! CoreDeviceProxy / RemotePairing tunnel + userspace TCP stack + RSD.
//!
//! The tunnel needs no admin rights and no TUN driver: `jktcp` (idevice
//! feature `tunnel_tcp_stack`) runs IPv6 TCP/UDP inside the process.
//! Dropping [`Tunnel`] stops the stack and every RSD service on it.

use std::net::SocketAddr;
use std::path::Path;

use ap_core::{Error, Result};
use idevice::{
    IdeviceService,
    core_device_proxy::CoreDeviceProxy,
    provider::IdeviceProvider,
    remote_pairing::{
        PeerDevice, RemotePairingClient, RpPairingFile, RpPairingSocket,
        connect_tls_psk_tunnel_native,
    },
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
};

use crate::errors::ctx;

/// RSD services Route B needs.
pub const SVC_DVT: &str = "com.apple.instruments.dtservicehub";
pub const SVC_DISPLAY: &str = "com.apple.coredevice.displayservice";
pub const SVC_INDIGO: &str = "com.apple.coredevice.hid.indigo";
pub const SVC_UHID: &str = "com.apple.coredevice.hid.universalhidservice";

pub const REQUIRED_SERVICES: &[&str] = &[SVC_DVT, SVC_DISPLAY, SVC_INDIGO, SVC_UHID];

/// A live tunnel with its RSD service list.
pub struct Tunnel {
    pub adapter: AdapterHandle,
    pub rsd: RsdHandshake,
}

impl Tunnel {
    pub fn has(&self, service: &str) -> bool {
        self.rsd.services.contains_key(service)
    }

    /// Required services the device does not offer.
    pub fn missing(&self) -> Vec<&'static str> {
        REQUIRED_SERVICES
            .iter()
            .copied()
            .filter(|s| !self.has(s))
            .collect()
    }
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel")
            .field("services", &self.rsd.services.len())
            .finish()
    }
}

async fn rsd_over(mut adapter: AdapterHandle, rsd_port: u16) -> Result<Tunnel> {
    let stream = adapter
        .connect(rsd_port)
        .await
        .map_err(|e| Error::NotConnected(format!("RSD connect: {e}")))?;
    let rsd = Box::pin(RsdHandshake::new(stream))
        .await
        .map_err(ctx("RSD handshake"))?;
    Ok(Tunnel { adapter, rsd })
}

/// Open CoreDeviceProxy over a lockdown provider (USB or Wi-Fi TCP).
/// iOS 17.4+ only.
pub async fn open_core_device_proxy(provider: &dyn IdeviceProvider) -> Result<Tunnel> {
    let proxy = Box::pin(CoreDeviceProxy::connect(provider))
        .await
        .map_err(ctx("CoreDeviceProxy tunnel (needs iOS 17.4+)"))?;
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let adapter = proxy
        .create_software_tunnel()
        .map_err(ctx("software tunnel"))?;
    Box::pin(rsd_over(adapter.to_async_handle(), rsd_port)).await
}

/// Open a RemotePairing tunnel to `addr` (`_remotepairing._tcp`) with a saved
/// RemotePairing file. Not verified on a device.
pub async fn open_remote_pairing(
    addr: SocketAddr,
    host_name: &str,
    pairing_path: &Path,
) -> Result<Tunnel> {
    let mut rpf = RpPairingFile::read_from_file(pairing_path)
        .await
        .map_err(ctx("read RemotePairing file"))?;
    let tcp = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|e| Error::NotConnected(format!("connect {addr}: {e}")))?;
    let mut rpc = RemotePairingClient::new(RpPairingSocket::new(tcp), host_name);
    // A PIN request means the saved file is no longer valid. Return an empty
    // PIN so the pairing fails instead of waiting for input.
    Box::pin(rpc.connect(&mut rpf, async || String::new()))
        .await
        .map_err(|e| {
            Error::setup(
                format!("Wi-Fi pairing was refused ({e})"),
                "connect the iPhone with a cable and save the Wi-Fi pairing again (`agentphone doctor`)",
            )
        })?;
    let port = rpc
        .create_tcp_listener()
        .await
        .map_err(ctx("RemotePairing createListener"))?;
    let mut tunnel_addr = addr;
    tunnel_addr.set_port(port);
    let tcp = tokio::net::TcpStream::connect(tunnel_addr)
        .await
        .map_err(|e| Error::NotConnected(format!("connect {tunnel_addr}: {e}")))?;
    let tunnel = Box::pin(connect_tls_psk_tunnel_native(tcp, rpc.encryption_key()))
        .await
        .map_err(ctx("TLS-PSK tunnel"))?;
    let client_ip = tunnel
        .info
        .client_address
        .parse()
        .map_err(|e| Error::protocol(format!("tunnel client address: {e}")))?;
    let server_ip = tunnel
        .info
        .server_address
        .parse()
        .map_err(|e| Error::protocol(format!("tunnel server address: {e}")))?;
    let mtu = tunnel.info.mtu as usize;
    let rsd_port = tunnel.info.server_rsd_port;
    let mut adapter =
        idevice::tcp::adapter::Adapter::new(Box::new(tunnel.into_inner()), client_ip, server_ip);
    adapter.set_mss(mtu.saturating_sub(60));
    Box::pin(rsd_over(adapter.to_async_handle(), rsd_port)).await
}

/// True when a `_remotepairing._tcp` TXT record belongs to the device that
/// `rpf` was paired with.
pub fn rp_matches(rpf: &RpPairingFile, identifier: &str, auth_tag: &str) -> bool {
    rpf.alt_irk()
        .is_some_and(|irk| PeerDevice::validate_auth_tag(irk, identifier, auth_tag))
}

/// Name this computer gives in RemotePairing.
pub fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "agentphone".into())
}

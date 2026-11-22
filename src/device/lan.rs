use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::net::IpAddr;

pub fn address() -> anyhow::Result<IpAddr> {
    if_addrs::get_if_addrs()?
        .into_iter()
        .map(|i| i.ip())
        .find(|ip| ip.is_ipv4() && !ip.is_loopback() && !ip.is_unspecified())
        .ok_or_else(|| anyhow::anyhow!("no LAN address available"))
}
pub fn advertise(server_id: &str, port: u16) -> anyhow::Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    let hostname = format!("tube2pod-{server_id}.local.");
    let properties = [("server_id", server_id)];
    let service = ServiceInfo::new(
        "_tube2pod._tcp.local.",
        server_id,
        &hostname,
        "",
        port,
        &properties[..],
    )?
    .enable_addr_auto();
    daemon.register(service)?;
    Ok(daemon)
}

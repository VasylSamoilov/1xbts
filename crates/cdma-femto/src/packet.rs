use crate::config::{DirectIpConfig, PacketAdapter, PacketConfig};
use std::net::IpAddr;

#[derive(Debug, thiserror::Error)]
pub enum PacketError {
    #[error("direct-IP adapter selected but no [packet.direct_ip] config present")]
    MissingDirectIpConfig,
}

pub struct FemtoPacketIngress {
    adapter: PacketAdapter,
    direct_ip: Option<DirectIpConfig>,
}

impl FemtoPacketIngress {
    pub fn new(config: &PacketConfig) -> Self {
        Self {
            adapter: config.adapter,
            direct_ip: config.direct_ip.clone(),
        }
    }

    pub fn adapter(&self) -> PacketAdapter {
        self.adapter
    }

    pub async fn start(&self) -> Result<(), PacketError> {
        match self.adapter {
            PacketAdapter::DirectIp => {
                let config = self
                    .direct_ip
                    .as_ref()
                    .ok_or(PacketError::MissingDirectIpConfig)?;
                self.serve_direct_ip(config)
            }
            PacketAdapter::ExternalPdsn => {
                log::info!("packet ingress delegated to external PDSN");
                std::future::pending::<Result<(), PacketError>>().await
            }
        }
    }

    fn serve_direct_ip(&self, config: &DirectIpConfig) -> Result<(), PacketError> {
        let policy = SourcePolicy::from_config(config);
        log::info!(
            "direct-IP adapter ready with {} allowed source range(s)",
            policy.ranges.len()
        );
        Ok(())
    }
}

pub struct SourcePolicy {
    ranges: Vec<(IpAddr, u8)>,
}

impl SourcePolicy {
    pub fn from_config(config: &DirectIpConfig) -> Self {
        let ranges = config
            .allowed_source_cidrs
            .iter()
            .filter_map(|cidr| parse_cidr(cidr))
            .collect();
        Self { ranges }
    }

    pub fn permits(&self, addr: IpAddr) -> bool {
        self.ranges
            .iter()
            .any(|(net, prefix)| cidr_contains(*net, *prefix, addr))
    }
}

fn parse_cidr(cidr: &str) -> Option<(IpAddr, u8)> {
    let (addr, prefix) = cidr.split_once('/')?;
    Some((addr.parse().ok()?, prefix.parse().ok()?))
}

fn cidr_contains(net: IpAddr, prefix: u8, addr: IpAddr) -> bool {
    match (net, addr) {
        (IpAddr::V4(net), IpAddr::V4(addr)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(prefix.min(32)))
            };
            (u32::from(net) & mask) == (u32::from(addr) & mask)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_ip_source_policy_admits_in_range_and_rejects_out_of_range() {
        let policy = SourcePolicy::from_config(&DirectIpConfig {
            allowed_source_cidrs: vec!["192.168.100.0/24".to_string()],
        });
        assert!(policy.permits("192.168.100.7".parse().unwrap()));
        assert!(!policy.permits("192.168.101.7".parse().unwrap()));
    }
}

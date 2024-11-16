use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::fmt::Display;
use std::error::Error;

pub mod asn1 {
    include!(concat!(env!("OUT_DIR"), "/asn1.rs"));
}

pub mod mtsid {
    include!(concat!(env!("OUT_DIR"), "/mtsid.rs"));
}

pub mod mediacontrol {
    include!(concat!(env!("OUT_DIR"), "/mediacontrol.rs"));
}

pub mod callsig {
    include!(concat!(env!("OUT_DIR"), "/callsig.rs"));
}

#[derive(Clone, Debug)]
pub enum PbIpAddrErr {
    VersionMissing,
    WrongLength,
}

impl Display for PbIpAddrErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PbIpAddrErr::WrongLength => f.write_str("wrong IP address length"),
            PbIpAddrErr::VersionMissing => f.write_str("IP version missing"),
        }
    }
}

impl Error for PbIpAddrErr {}

#[derive(Clone, Debug)]
pub enum PbTransportAddrErr {
    VariantMissing,
    PortNumberTooLarge,
    IpAddr(PbIpAddrErr),
    Hostname(String, u16),
}

impl Display for PbTransportAddrErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PbTransportAddrErr::VariantMissing => f.write_str("transport address variant missing"),
            PbTransportAddrErr::PortNumberTooLarge => f.write_str("port number too large"),
            PbTransportAddrErr::IpAddr(ip) => ip.fmt(f),
            PbTransportAddrErr::Hostname(hn, port) => f.write_str("hostname used instead of IP"),
        }
    }
}

impl Error for PbTransportAddrErr {}

impl From<PbIpAddrErr> for PbTransportAddrErr {

    fn from(value: PbIpAddrErr) -> Self {
        PbTransportAddrErr::IpAddr(value)
    }

}

pub fn pbip_to_netip (ip: &mediacontrol::IpAddress) -> Result<std::net::IpAddr, PbIpAddrErr> {
    if ip.version.is_none() {
        return Err(PbIpAddrErr::VersionMissing);
    }
    match ip.version.as_ref().unwrap() {
        mediacontrol::ip_address::Version::V4(v4) => {
            if v4.len() != 4 {
                return Err(PbIpAddrErr::WrongLength);
            }
            let ipv4 = Ipv4Addr::new(v4[0], v4[1], v4[2], v4[3]);
            Ok(IpAddr::V4(ipv4))
        },
        mediacontrol::ip_address::Version::V6(v6) => {
            if v6.len() != 16 {
                return Err(PbIpAddrErr::WrongLength);
            }
            let ipv6 = Ipv6Addr::from(
                u128::from_be_bytes([
                    v6[0],
                    v6[1],
                    v6[2],
                    v6[3],
                    v6[4],
                    v6[5],
                    v6[6],
                    v6[7],
                    v6[8],
                    v6[9],
                    v6[10],
                    v6[11],
                    v6[12],
                    v6[13],
                    v6[14],
                    v6[15],
                ])
            );
            Ok(IpAddr::V6(ipv6))
        }
    }
}

pub fn netip_to_pbip (ip: &std::net::IpAddr) -> mediacontrol::IpAddress {
    match ip {
        IpAddr::V4(v4) => {
            mediacontrol::IpAddress{
                version: Some(mediacontrol::ip_address::Version::V4(v4.octets().to_vec()))
            }
        },
        IpAddr::V6(v6) => {
            mediacontrol::IpAddress{
                version: Some(mediacontrol::ip_address::Version::V6(v6.octets().to_vec()))
            }
        },
    }
}

pub fn socketaddr_to_transportaddr (addr: &std::net::SocketAddr) -> mediacontrol::TransportAddress {
    let ip = addr.ip();
    let pbip = netip_to_pbip(&ip);
    mediacontrol::TransportAddress{
        variant: Some(mediacontrol::transport_address::Variant::IpAddress(pbip)),
        port: addr.port() as u32,
        multicast: ip.is_multicast(),
    }
}

pub fn transportaddr_to_socketaddr (addr: &mediacontrol::TransportAddress) -> Result<std::net::SocketAddr, PbTransportAddrErr> {
    if addr.variant.is_none() {
        return Err(PbTransportAddrErr::VariantMissing);
    }
    let port = addr.port;
    if port > u16::MAX as u32 {
        return Err(PbTransportAddrErr::PortNumberTooLarge);
    }
    let variant = addr.variant.as_ref().unwrap();
    match variant {
        mediacontrol::transport_address::Variant::Hostname(hn) => {
            return Err(PbTransportAddrErr::Hostname(hn.to_owned(), port as u16))
        },
        mediacontrol::transport_address::Variant::IpAddress(ip) => {
            let ip = pbip_to_netip(&ip)?;
            Ok(std::net::SocketAddr::new(ip, port as u16))
        },
    }
}

use rsip::typed::CSeq;
use tokio::task::AbortHandle;
use tokio::time::Sleep;
use uuid::Uuid;
use std::hash::Hash;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, atomic::AtomicU32};
use tokio::sync::{Mutex, RwLock};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};
use tokio_util::sync::{CancellationToken, DropGuard};

pub type LogicalChannelNumber = u32;
pub type CallIdentifier = rsip::headers::CallId;

const DEFAULT_SIP_PORT: u16 = 5060;
const DEFAULT_SIPS_PORT: u16 = 5061;
const DEFAULT_INTERFACE: IpAddr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));

#[derive(Debug, Clone)]
pub struct RTPStreamState {
    pub stream_start_time: Instant,
    pub initial_timestamp: u32,
    pub initial_ssrc: u32,
    pub expected_sequence_number: u16,
    pub canceller: CancellationToken,
}

#[derive(Debug, Clone)]
pub struct ChannelState {
    // pub olc: OpenLogicalChannel,
    pub recv: RTPStreamState,

    /// If this program is a gateway, this is the UDP port opened up by the
    /// called party to which the RTP packets are to be forwarded.
    pub forward_to_port: Option<u16>,
}

#[derive(Debug, Clone)]
pub enum CallStatus {
    NotStarted,
    WaitingForAck(CSeq),
    InProgress,
    Ended,
}

impl CallStatus {

    pub fn is_waiting (&self) -> bool {
        match self {
            CallStatus::WaitingForAck(_) => true,
            _ => false,
        }
    }

}

#[derive(Debug)]
pub struct CallState {
    pub id: CallIdentifier,
    pub channels: HashMap<LogicalChannelNumber, ChannelState>,
    // pub capabilities: Option<TerminalCapabilitySet>,
    pub expected_message_id: Arc<AtomicU32>, // TODO: Does this need to be here?

    /// If this program is a gateway, this is the IP address of the called
    /// party, saved so that call signalling messages can be passed on to them.
    pub forward_to_ip: Option<IpAddr>,

    pub status: CallStatus,

    pub drop_guards: Vec<DropGuard>,
}

impl CallState {

    pub fn new (id: CallIdentifier) -> Self {
        CallState{
            id,
            channels: HashMap::new(),
            expected_message_id: Arc::new(AtomicU32::new(0)),
            forward_to_ip: None,
            status: CallStatus::NotStarted,
            drop_guards: vec![],
        }
    }

}

#[derive(Debug, Clone)]
pub struct PeerState {
    // pub last_msg: Instant,
    pub addr: SocketAddr,
    // TODO: String is the CallId, but rsip::CallId does not implement Hash
    pub calls: Arc<RwLock<HashMap<String, Arc<Mutex<CallState>>>>>,
    pub impending_doom: Option<AbortHandle>,
}

impl PeerState {

    pub fn new (addr: SocketAddr) -> Self {
        PeerState{
            addr,
            calls: Arc::new(RwLock::new(HashMap::new())),
            impending_doom: None,
        }
    }

}


#[derive(Debug, Clone)]
pub struct ServerState {
    pub peers: Arc<RwLock<HashMap<SocketAddr, PeerState>>>,
    pub is_busy: Arc<AtomicBool>,
    pub config: Arc<Config>,
}

// #[derive(Debug, Clone)]
// pub enum RoutableAddress {
//     Ip(SocketAddr),
//     HostAndPort(String, u16),
// }

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PortConfig {
    pub interface: Option<IpAddr>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct CIDRAddress {
    pub ip: IpAddr,
    pub prefix: u8,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Config {
    pub sip_tcp: Option<PortConfig>,
    pub sip_udp: Option<PortConfig>,

    pub local_name: Option<String>,
    pub local_display_name: Option<String>,
    pub public_dns_name: Option<String>,
    pub private_dns_name: Option<String>,
    #[serde(default)]
    pub private_network_ips: Vec<CIDRAddress>,

    // This is for IP address discovery.
    // TODO: Use this client: https://crates.io/crates/stunclient
    #[serde(default)]
    pub stun_servers: Vec<String>,

    #[serde(default)]
    pub inactivity_timeout: u32,
}

impl Config {

    pub fn sip_udp_port (&self) -> u16 {
        self.sip_udp
            .as_ref()
            .map(|p| p.port.unwrap_or(DEFAULT_SIP_PORT))
            .unwrap_or(DEFAULT_SIP_PORT)
    }

    pub fn sip_tcp_port (&self) -> u16 {
        self.sip_tcp
            .as_ref()
            .map(|p| p.port.unwrap_or(DEFAULT_SIP_PORT))
            .unwrap_or(DEFAULT_SIP_PORT)
    }

    pub fn sip_udp_interface (&self) -> IpAddr {
        self.sip_udp
            .as_ref()
            .map(|p| p.interface.unwrap_or(DEFAULT_INTERFACE))
            .unwrap_or(DEFAULT_INTERFACE)
    }

    pub fn sip_tcp_interface (&self) -> IpAddr {
        self.sip_tcp
            .as_ref()
            .map(|p| p.interface.unwrap_or(DEFAULT_INTERFACE))
            .unwrap_or(DEFAULT_INTERFACE)
    }

    pub fn sip_udp_address (&self) -> SocketAddr {
        SocketAddr::new(
            self.sip_udp_interface(),
            self.sip_udp_port(),
        )
    }

    pub fn sip_tcp_address (&self) -> SocketAddr {
        SocketAddr::new(
            self.sip_tcp_interface(),
            self.sip_tcp_port(),
        )
    }

}

impl Default for Config {

    fn default() -> Self {
        Config {
            sip_tcp: None,
            sip_udp: None,
            local_name: None,
            inactivity_timeout: 5,
            private_dns_name: None,
            public_dns_name: None,
            private_network_ips: vec![],
            stun_servers: vec![],
            local_display_name: None,
        }
    }
    
}
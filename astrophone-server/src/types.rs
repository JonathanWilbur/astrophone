use uuid::Uuid;
use std::sync::{Arc, atomic::AtomicU32};
use tokio::sync::{Mutex, RwLock};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use crate::p::mediacontrol::{TerminalCapabilitySet, OpenLogicalChannel};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub type LogicalChannelNumber = u32;
pub type CallIdentifier = Uuid;

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
    pub olc: OpenLogicalChannel,
    pub recv: RTPStreamState,

    /// If this program is a gateway, this is the UDP port opened up by the
    /// called party to which the RTP packets are to be forwarded.
    pub forward_to_port: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct CallState {
    pub id: CallIdentifier,
    pub local_is_master: bool,
    pub channels: HashMap<LogicalChannelNumber, ChannelState>,
    pub capabilities: Option<TerminalCapabilitySet>,
    pub expected_message_id: Arc<AtomicU32>,

    /// If this program is a gateway, this is the IP address of the called
    /// party, saved so that call signalling messages can be passed on to them.
    pub forward_to_ip: Option<IpAddr>,
}

#[derive(Debug, Clone)]
pub struct PeerState {
    pub addr: SocketAddr,
    pub calls: Arc<RwLock<HashMap<CallIdentifier, Arc<Mutex<CallState>>>>>,
}


#[derive(Debug, Clone)]
pub struct ServerState {
    pub peers: Arc<RwLock<HashMap<SocketAddr, PeerState>>>,
}

#[derive(Debug, Clone)]
pub enum RoutableAddress {
    Ip(SocketAddr),
    HostAndPort(String, u16),
}

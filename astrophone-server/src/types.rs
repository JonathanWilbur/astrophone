use rsip::typed::CSeq;
use uuid::Uuid;
use std::hash::Hash;
use std::sync::{Arc, atomic::AtomicU32};
use tokio::sync::{Mutex, RwLock};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Instant;
use tokio_util::sync::{CancellationToken, DropGuard};

pub type LogicalChannelNumber = u32;
pub type CallIdentifier = rsip::headers::CallId;

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
    pub addr: SocketAddr,
    // TODO: String is the CallId, but rsip::CallId does not implement Hash
    pub calls: Arc<RwLock<HashMap<String, Arc<Mutex<CallState>>>>>,
}

impl PeerState {

    pub fn new (addr: SocketAddr) -> Self {
        PeerState{
            addr,
            calls: Arc::new(RwLock::new(HashMap::new())),
        }
    }

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

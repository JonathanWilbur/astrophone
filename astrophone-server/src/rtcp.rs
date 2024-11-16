pub const RTCP_PT_SR: u8    = 200; // Sender Report
pub const RTCP_PT_RR: u8    = 201; // Receiver Report
pub const RTCP_PT_SDES: u8  = 202; // Source Descriptor
pub const RTCP_PT_BYE: u8   = 203; // Goodbye
pub const RTCP_PT_APP: u8   = 204; // Application-Defined

pub const RTCP_SDES_CNAME: u8 = 1; // Deterministic identifier formatted like an email address
pub const RTCP_SDES_NAME: u8 = 2; // Real name
pub const RTCP_SDES_EMAIL: u8 = 3; // Email address
pub const RTCP_SDES_PHONE: u8 = 4; // Phone Number
pub const RTCP_SDES_LOC: u8 = 5; // Geographic Location as a String
pub const RTCP_SDES_TOOL: u8 = 6; // Tool Name
pub const RTCP_SDES_NOTICE: u8 = 7; // Notice / Status String
pub const RTCP_SDES_PRIV: u8 = 8; // Private Extension

#[derive(Clone, Debug, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct RTCPHeader {
    pub flags: u8,
    pub packet_type: u8,
    pub length: u16,
    pub ssrc: u32,
}

#[derive(Clone, Debug, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct SenderReportHeader {
    pub ntp_timestamp: [u8; 8],
    pub rtp_timestamp: u32,
    pub sender_packet_count: u32,
    pub sender_octet_count: u32,
}

// The sender report and receiver report blocks have the same format.
#[derive(Clone, Debug, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct RTCPReportBlock {
    pub ssrc: u32,
    pub fraction_lost: u8,
    pub cumulative_packet_loss: [u8; 3],
    pub ext_highest_seqnum_received: u32,
    pub interarrival_jitter: u32,
    pub last_sender_report: u32,
    pub delay_since_last_sender_report: u32,
}

// It is just an array of SSRC / CSRC values.
// #[derive(Clone, Debug, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
// #[repr(C)]
// pub struct RTCPGoodbye {

// }

#[derive(Clone, Debug, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct RTCPAppData {
    pub name: [u8; 4],
}

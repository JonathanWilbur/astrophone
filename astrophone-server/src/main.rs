mod logging;
mod rtcp;
mod types;
use crate::logging::get_default_log4rs_config;
use ringbuf::SharedRb;
use rsip::headers::{AcceptEncoding, AcceptLanguage, ContentLength, Server, Supported};
use rsip::prelude::{HasHeaders, HeadersExt, ToTypedHeader, UntypedHeader};
use rsip::typed::content_disposition::DisplayType;
use rtcp::{RTCPHeader, SenderReportHeader};
use sdp_rs::lines::connection::ConnectionAddress;
use sdp_rs::lines::media::ProtoType;
use sdp_rs::lines::{Attribute, Connection, Media, Origin, Version};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tokio::task::AbortHandle;
use tokio::time::sleep;
use types::{CallStatus, RTPStreamState};
use core::str;
use std::io::{self, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use std::time::{Duration, Instant};
use anyhow;
use std::collections::HashMap;
use uuid::Uuid;
use tokio::sync::{Mutex, mpsc};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio_util::sync::{CancellationToken, DropGuard};
use crate::types::{PeerState, ChannelState, CallState, ServerState};
use audio_codec_algorithms::decode_alaw;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate};
use ringbuf::{
    traits::{Consumer, Producer, Split},
    HeapRb,
    CachingProd,
};
use ringbuf::storage::Heap;
use std::mem::size_of;
use rtcp::{
    RTCP_PT_SR,
    RTCP_PT_RR,
    RTCP_PT_SDES,
    RTCP_PT_BYE,
    RTCP_PT_APP,
    RTCP_SDES_CNAME,
    RTCP_SDES_NAME,
    RTCP_SDES_EMAIL,
    RTCP_SDES_PHONE,
    RTCP_SDES_LOC,
    RTCP_SDES_TOOL,
    RTCP_SDES_NOTICE,
    RTCP_SDES_PRIV,
};
use tokio::net::TcpListener;
use rsip::{
    Domain, Header, Headers, Host, Method, Port, Request, Response, StatusCode, Transport
};
use rsip::typed::{Accept, Allow, CSeq, Contact, ContentDisposition, ContentType, MediaType, Via};
use local_ip_address::local_ip;
use sdp_rs::{MediaDescription, SessionDescription, Time};
use tokio::fs::read_to_string;
use std::path::PathBuf;
use types::Config;

const MAX_CALLS_PER_PEER: usize = 3;
const ALLOWED_METHODS: [rsip::Method; 5] = [
    Method::Invite,
    Method::Ack,
    Method::Bye,
    Method::Cancel,
    Method::Options,
];

const SUPPORTED_OPTIONS: &str = "";
const ACCEPT_ENCODING: &str = ""; // empty means "identity" encoding only.
const ACCEPT_LANGUAGE: &str = "en";
const SERVER: &str = "Astrophone (See https://github.com/JonathanWilbur/astrophone)";

/// Source: https://datatracker.ietf.org/doc/html/rfc3261#section-17.1.1.1
const DEFAULT_T1_MILLISECONDS: u32 = 500;

/// Source: https://datatracker.ietf.org/doc/html/rfc3261#section-17.1.2.2
const DEFAULT_T2_MILLISECONDS: u32 = 4000;

#[cfg(not(target_family = "unix"))]
const CONFIG_LOCATIONS: [ &str; 1 ] = [
    "./astrophone",
];
#[cfg(target_family = "unix")]
const CONFIG_LOCATIONS: [ &str; 2 ] = [
    "./astrophone",
    "/etc/astrophone",
];

const CONFIG_SUFFIXES: [ &str; 2 ] = [
    ".toml",
    ".config.toml",
];

fn handle_rtp_packet <'a> (
    rtp_packet: &'a rtp_rs::RtpReader<'a>,
) {
    // if rtp_packet.version() != 2 { // TODO: Handle version 1?
    //     return;
    // }
    // TODO: If this is the first RTP packet, record the initial timestamp and sequence number.
    // (since the timestamp can be random)
    // TODO: Record the SSRC. Ignore subsequent packets that differ in this regard.
    // TODO: If the RTP packet is due to be played now, play it now, otherwise, schedule it in the future.

    // For now, just assume the payload is G.711A. I'll make it nice later.
    // rtp_packet.payload()
}

async fn receive_rtp_packets (
    correct_peer_addr: SocketAddr,
    socket: UdpSocket,
    rtcp_socket: UdpSocket,
    cancel: CancellationToken,
    mut producer: CachingProd<Arc<SharedRb<Heap<u8>>>>,
    play_flag: Arc<AtomicBool>,
) {
    let mut rtcp_buf = [0; 65536];
    let mut rtp_buf = [0; 65536];
    let mut playing = false;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                break;
            }
            val = socket.recv_from(&mut rtp_buf) => {
                let (len, peer_addr) = val.unwrap(); // TODO: Handle
                if peer_addr.ip() != correct_peer_addr.ip() {
                    log::warn!("unauthorized RTP packet from {}, but owner is {}", peer_addr, correct_peer_addr);
                    continue; // Just ignore packets from naughtybois.
                }
                match rtp_rs::RtpReader::new(&rtp_buf[0..len]) {
                    Ok(rtp_packet) => {
                        if !playing {
                            play_flag.store(true, Ordering::Relaxed);
                            playing = true;
                        }
                        // TODO: Trigger playback
                        // TODO: Check the stream number and such from the packet.
                        // handle_rtp_packet(&rtp_packet);
                        producer.push_slice(rtp_packet.payload());
                    },
                    Err(e) => {
                        log::warn!("Error decoding RTP packet: {:?}", e); // TODO: Handle this some other way.
                    },
                };
            },
            val = rtcp_socket.recv_from(&mut rtcp_buf) => {
                let (len, peer_addr) = val.unwrap(); // TODO: Handle
                if peer_addr.ip() != correct_peer_addr.ip() {
                    log::warn!("unauthorized RTP packet from {}, but owner is {}", peer_addr, correct_peer_addr);
                    continue; // Just ignore packets from naughtybois.
                }
                if len < 8 {
                    if !(len == 4 && rtcp_buf[1] == RTCP_PT_BYE) {
                        log::debug!("Empty BYE RTCP packet received from {}", peer_addr);
                        continue;
                    }
                    log::warn!("Invalid RTCP packet from {}", peer_addr);
                    continue;
                }
                if (rtcp_buf[0] & 0b1100_0000) != 0b1000_0000 {
                    log::warn!("Unsupported RTCP packet version from peer {} (first byte was {:#04X})", peer_addr, rtcp_buf[0]);
                    continue;
                }
                // TODO: Check version
                // This should be aligned, since it is 8 bytes and starts at the
                // beginning of the stack frame.
                let rtcp_header: RTCPHeader = bytemuck::pod_read_unaligned(&rtcp_buf[0..size_of::<RTCPHeader>()]);
                
                log::debug!("Received RTCP packet type {} of length {} for SSRC {:#010X} from peer {}",
                    rtcp_header.packet_type,
                    rtcp_header.length,
                    rtcp_header.ssrc,
                    peer_addr);
                // TODO: Move parsing logic to separate functions for testability.
                match rtcp_header.packet_type {
                    RTCP_PT_SR => { // Sender Report https://datatracker.ietf.org/doc/html/rfc3550#section-6.4.1
                        // let sender_report_header: SenderReportHeader = bytemuck::pod_read_unaligned(
                        //     &rtcp_buf[8..8+size_of::<SenderReportHeader>()]);
                        // TODO: Use
                    },
                    RTCP_PT_RR => { // Receiver Report https://datatracker.ietf.org/doc/html/rfc3550#section-6.4.2
                        // TODO: Use
                    },
                    RTCP_PT_SDES => { // Source Description https://datatracker.ietf.org/doc/html/rfc3550#section-6.5
                        let reported_count = rtcp_buf[0] & 0b0001_1111;
                        let mut actual_count = 1;
                        let mut ssrc: Option<u32> = Some(rtcp_header.ssrc);
                        let mut s = &rtcp_buf[8..];
                        while s.len() > 0 {
                            if s[0] == 0 {
                                ssrc = None;
                                continue;
                            }
                            if ssrc.is_none() {
                                if s.len() < 4 {
                                    log::warn!("Malformed (short) SSRC in SDES RTCP packet from peer {}", peer_addr);
                                    break;
                                }
                                ssrc = Some(u32::from_be_bytes([ s[0], s[1], s[2], s[3] ]));
                                actual_count += 1;
                                s = &s[4..];
                                continue;
                            }
                            let sdes_type = s[0];
                            s = &s[1..];
                            if s.len() == 0 {
                                log::warn!("Malformed (short) SSRC in SDES RTCP packet from peer {}", peer_addr);
                                break;
                            }
                            let sdes_len = s[0] as usize;
                            s = &s[1..];
                            if s.len() < sdes_len {
                                log::warn!("Malformed (short) SSRC in SDES RTCP packet from peer {}", peer_addr);
                                break;
                            }
                            let sdes = &s[0..sdes_len];
                            let sdes_name = match sdes_type {
                                // all ascii / utf8 except PRIV
                                RTCP_SDES_CNAME => "CNAME",
                                RTCP_SDES_NAME => "NAME",
                                RTCP_SDES_EMAIL => "EMAIL",
                                RTCP_SDES_PHONE => "PHONE",
                                RTCP_SDES_LOC => "LOC",
                                RTCP_SDES_TOOL => "TOOL",
                                RTCP_SDES_NOTICE => "NOTICE",
                                RTCP_SDES_PRIV => "PRIV",
                                _ => "?",
                            };
                            if sdes_type < 8 {
                                let sdes_str = match str::from_utf8(sdes) {
                                    Ok(ss) => ss,
                                    Err(_) => {
                                        continue;
                                    },
                                };
                                // TODO: Validate that there are no control characters.
                                log::debug!("SDES info from peer rtcp://{}: {}={}", peer_addr, sdes_name, sdes_str);
                            }
                            s = &s[sdes_len..];
                            actual_count += 1;
                        }
                        if actual_count != reported_count {
                            log::warn!("The reported source count in SDES packet sent by peer rtcp://{} does not match the actual source count contained", peer_addr);
                        }
                        // TODO: Use
                    },
                    RTCP_PT_BYE => { // Goodbye
                        let ssrc_count = rtcp_header.flags & 0b0001_1111;
                        if len < (ssrc_count as usize * 4) + 4 {
                            log::warn!("Malformed (short) BYE RTCP packet from peer {}", peer_addr);
                            continue;
                        }
                        let mut i = 4;
                        while i < len {
                            let ssrc: u32 = u32::from_be_bytes([
                                rtcp_buf[i], rtcp_buf[i+1], rtcp_buf[i+2], rtcp_buf[i+3] ]);
                            log::info!("Goodbye received for SSRC {} from rtcp://{}", ssrc, peer_addr);
                            i += 4;
                        }
                        if len > (ssrc_count as usize * 4) + 5 { // If we have a reason string
                            let start_of_reason = (ssrc_count as usize * 4) + 5;
                            let reason_len = std::cmp::min( // Take the lesser of the reported length or packet length
                                rtcp_buf[(ssrc_count as usize * 4) + 4] as usize, // reason length field
                                len - start_of_reason // length of packet, minus everything before reason
                            );
                            let reason = match str::from_utf8(&rtcp_buf[start_of_reason..start_of_reason+reason_len]) {
                                Ok(s) => s,
                                Err(_) => {
                                    log::warn!("Invalid reason string in BYE RTCP packet from peer {}", peer_addr);
                                    continue;
                                },
                            };
                            log::info!("BYE RTCP packet received from peer {} for reason \"{}\"", peer_addr, reason);
                        }
                    },
                    RTCP_PT_APP => { // Application Data
                        if len < 12 {
                            log::warn!("Malformed (short) APP RTCP packet received from peer {}", peer_addr);
                            continue;
                        }
                        let subtype = rtcp_header.flags & 0b0001_1111;
                        let name_bytes = &rtcp_buf[8..12];
                        if !name_bytes.iter().all(|b| (*b).is_ascii_graphic()) {
                            log::warn!("Peer {} sent an APP RTCP packet with a name field that is not graphical ASCII", peer_addr);
                            continue;
                        }
                        let name = str::from_utf8(name_bytes).expect("Graphical ASCII was not valid UTF-8 apparently!");
                        log::warn!("Unrecognized APP RTCP packet with subtype {} and name {} from peer {}",
                            subtype, name, peer_addr);
                    },
                    _ => {
                        log::debug!("Unrecognized RTCP packet type {} from peer {}",
                            rtcp_header.packet_type, peer_addr);
                    }
                };
            }
        }
    };
}

async fn listen_for_rtp (
    server: ServerState,
    peer_addr: SocketAddr,
) -> anyhow::Result<(u16, u16, CancellationToken)> {
    let port = 0; // This means "give me any port."
    let ip = Ipv4Addr::new(0, 0, 0, 0); // TODO: Make this configurable.
    let socket_addr = SocketAddr::new(ip.into(), port); 
    let socket = UdpSocket::bind(socket_addr).await?;

    let assigned_port = socket.local_addr()?.port();

    let rtcp_socket: UdpSocket;
    /* "For UDP and similar protocols, RTP SHOULD use an even destination port
    number and the corresponding RTCP stream SHOULD use the next higher (odd)
    destination port number." -- IETF RFC 3550 */
    let rtp_socket = if (assigned_port % 2) == 1 { // If the assigned port is odd
        log::info!("Listening for RTCP traffic on {}", socket_addr);
        rtcp_socket = socket;
        let socket_addr = SocketAddr::new(ip.into(), assigned_port - 1);
        let rtp_sock = UdpSocket::bind(socket_addr).await?;
        log::info!("Listening for RTP traffic on {}", socket_addr);
        rtp_sock
    } else {
        log::info!("Listening for RTP traffic on {}", socket_addr);
        let socket_addr = SocketAddr::new(ip.into(), assigned_port + 1);
        rtcp_socket = UdpSocket::bind(socket_addr).await?;
        log::info!("Listening for RTCP traffic on {}", socket_addr);
        socket
    };

    let rtp_port = rtp_socket.local_addr()?.port();
    let rtcp_port = rtcp_socket.local_addr()?.port();
    
    let token = CancellationToken::new();

    let ring = HeapRb::<u8>::new(24000);
    let (producer, mut consumer) = ring.split();

    let host = cpal::default_host();
    let device = host.default_output_device().expect("No output audio device found");
    let mut supported_configs = device.supported_output_configs()
        .expect("error while querying configs");
    let config = supported_configs
        .find(|c| c.channels() == 1 && c.sample_format() == SampleFormat::I16);
    let config = config.unwrap().with_sample_rate(SampleRate(8000)).into();
    let play_flag = Arc::new(AtomicBool::new(false));
    tokio::spawn(receive_rtp_packets(
        peer_addr,
        rtp_socket,
        rtcp_socket,
        token.clone(),
        producer,
        play_flag.clone(),
    ));

    /* If this approach seems abstruse, let me assure you I did not want to do
    it this way. See: https://github.com/RustAudio/cpal/issues/818 */
    let flag = Arc::new(AtomicBool::new(false));
    let flag2 = Arc::clone(&flag);
    let play_flag2 = play_flag.clone();
    let thread2 = std::thread::spawn(move || {
        let stream = device.build_output_stream(
            &config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                let mut input_fell_behind = false;
                for sample in data {
                    *sample = match consumer.try_pop() {
                        Some(s) => decode_alaw(s),
                        None => {
                            input_fell_behind = true;
                            i16::MAX - 10000
                        }
                    };
                }
                if input_fell_behind {
                    log::error!("input stream fell behind: try increasing latency");
                }
            },
            move |err| {
                log::error!("Audio output error: {}", err);
            },
            // None,
            Some(Duration::from_millis(30)) // None=blocking, Some(Duration)=timeout
        ).expect("Could not build output stream");
        // TODO: Use play_flag?
        // std::thread::sleep(Duration::from_millis(5000));
        stream.play().expect("Could not start audio output");
        while !flag2.load(Ordering::Relaxed) {
            std::thread::park();
        }
    });

    // This next section is basically:
    // "Stop the audio output if the RTP stream is closed."
    let token2 = token.clone();
    tokio::spawn(async move {
        token2.cancelled().await; // Wait until cancellation
        flag.store(true, Ordering::Relaxed); // Set a flag to true (atomically)
        // Finally, unblock the audio streaming thread, so the `stream` value
        // can drop and thereby stop outputting audio.
        thread2.thread().unpark();
        /* Implementation decision: since the resource that is actually a matter
        of contention on this server is the audio output, exiting the busy state
        will be tied to the closing of the audio stream. */
        server.is_busy.store(false, Ordering::Relaxed);
    });

    Ok((rtp_port, rtcp_port, token))
}

fn make_general_response (
    req: &Request,
    status_code: StatusCode,
) -> anyhow::Result<Vec<u8>> {
    // See: https://datatracker.ietf.org/doc/html/rfc3261#section-8.1.1
    // A valid SIP request formulated by a UAC MUST, at a minimum, contain
    // the following header fields: To, From, CSeq, Call-ID, Max-Forwards,
    // and Via; all of these header fields are mandatory in all SIP
    // requests.
    let call_id = req.call_id_header()?;
    let cseq = req.cseq_header()?;
    let from = req.from_header()?;
    let to = req.to_header()?;
    let via = req.via_header()?;
    let mut headers = Headers::default();
    headers.push(Header::CallId(call_id.to_owned()));
    headers.push(Header::CSeq(cseq.to_owned()));
    headers.push(Header::From(from.to_owned()));
    headers.push(Header::To(to.to_owned()));
    headers.push(Header::ContentLength(ContentLength::new("0")));
    // This is the correct procedure, as long as you are not forwarding, in which
    // case, you will need to add another Via header.
    headers.push(via.clone().into());
    Ok(Response{
        status_code,
        version: rsip::Version::V2,
        headers,
        body: vec![],
        ..Default::default()
    }.to_string().into_bytes())
}

/// This function determines whether an IP address should be considered "local"
/// for the purposes of returning the Contact header.
fn is_local_ip (ip: IpAddr) -> bool {
    // TODO: Check if it falls within the configured private CIDR subnets.
    match ip {
        IpAddr::V4(v4) => v4.is_private()
            || v4.is_broadcast()
            || v4.is_documentation()
            || v4.is_link_local()
            || v4.is_loopback()
            || v4.is_multicast()
            || v4.is_unspecified(),
        IpAddr::V6(v6) => is_local_ip(v6.to_canonical())
            || v6.is_loopback()
            || v6.is_multicast()
            || v6.is_unspecified()
    }
}

async fn get_contact_uri (
    server: &ServerState,
    peer_addr: SocketAddr,
    transport: Transport,
) -> rsip::Uri {
    let is_local_peer = is_local_ip(peer_addr.ip());
    let config_addr = match transport {
        Transport::Udp => Some(server.config.sip_udp_address()),
        Transport::Tcp => Some(server.config.sip_tcp_address()),
        _ => None,
    };
    // If we configured the IP address to "unspecified" (0.0.0.0 or ::),
    // we just pretend that we configured no IP address.
    let config_addr = config_addr
        .and_then(|addr| if addr.ip().is_unspecified() { None } else { Some(addr) });
    let my_host = if is_local_peer {
        if let Some(dns_name) = &server.config.private_dns_name {
            Host::Domain(Domain::new(dns_name))
        } else {
            let my_local_ip = config_addr
                .map(|a| a.ip())
                .or(local_ip().ok())
                .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
            Host::IpAddr(my_local_ip)
        }
    } else {
        if let Some(dns_name) = &server.config.public_dns_name {
            Host::Domain(Domain::new(dns_name))
        } else {
            // TODO: Cache the answer.
            let public_ip = public_ip::addr().await;
            let my_public_ip = config_addr
                .map(|a| a.ip())
                .or(public_ip)
                .or(local_ip().ok())
                .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
                ;
            Host::IpAddr(my_public_ip)
        }
    };
    let port = match transport {
        Transport::Udp => Some(server.config.sip_udp_port()),
        Transport::Tcp => Some(server.config.sip_tcp_port()),
        _ => None
    };

    // TODO: When TLS is supported, prefer the SIPS URI, if configured.
    let base_uri = rsip::Uri {
        scheme: Some(rsip::Scheme::Sip),
        auth: server.config.local_name.as_ref().map(|ln| (ln, Option::<String>::None).into()),
        host_with_port: rsip::HostWithPort::from((my_host, port)),
        ..Default::default()
    };
    base_uri
}

async fn get_contact (
    server: &ServerState,
    peer_addr: SocketAddr,
    transport: Transport,
) -> Contact {
    let contact_uri = get_contact_uri(server, peer_addr, transport).await;
    Contact{
        display_name: server.config.local_display_name.as_ref().map(|n| n.into()),
        uri: contact_uri.clone(),
        params: vec![],
    }
}

// async fn get_connection_ip (
//     server: &ServerState,
//     peer_addr: SocketAddr,
//     transport: Transport,
// ) -> IpAddr {
//     let is_local_peer = is_local_ip(peer_addr.ip());
//     let config_ip = match transport {
//         Transport::Udp => Some(server.config.sip_udp_interface()),
//         Transport::Tcp => Some(server.config.sip_tcp_interface()),
//         _ => None,
//     };
// }

async fn handle_ack (
    server: ServerState,
    req: Request,
    peer: PeerState,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    transport: Transport,
) -> anyhow::Result<()> {
    let call_id = req.call_id_header()?;
    let cseq = req.cseq_header()?;
    // let from = req.from_header()?;
    // let to = req.to_header()?;
    // let via = req.via_header()?;
    let calls = peer.calls.read().await;
    let maybe_call = calls.get(&call_id.to_string());
    let call = match maybe_call {
        Some(c) => c,
        None => {
            log::warn!("No such call with identifier {}", call_id.to_string());
            // TODO: Send error response.
            return Ok(());
        }
    };
    let mut call = call.lock().await;
    match &call.status {
        CallStatus::WaitingForAck(expected_cseq) => {
            if cseq.typed().unwrap() != *expected_cseq { // TODO: Handle error
                // TODO: Send error response
            }
            call.status = CallStatus::InProgress;
            // TODO: Set up RTP stream.
            // listen_for_rtp(peer.addr).await?;
        },
        _ => {}, // TODO: Other scenarios need to be handled too.
    };
    Ok(())
    // drop(calls);
}

///  From IETF RFC 3261:
///
///  > In this specification, the BYE method terminates a session and the dialog
///  > associated with it.
/// 
async fn handle_bye (
    server: ServerState,
    req: Request,
    peer: PeerState,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
) -> anyhow::Result<bool> {
    let call_id = req.call_id_header()?;
    let mut calls = peer.calls.write().await;
    let maybe_call = calls.remove(&call_id.to_string());
    if maybe_call.is_none() {
        log::warn!("No such call with identifier {}", call_id.to_string());
        let resp = make_general_response(&req, StatusCode::CallTransactionDoesNotExist)?;
        tx.send((resp, peer.addr)).await?;
        return Ok(false);
    };
    /* In theory, nothing else should be needed to drop the resources used by
    the call. We use DropGuards in CallState so that the RTP sockets get closed
    when the call is ended. */
    // Delete PeerState if this is the last call.
    let peers = peer.calls.read().await;
    let forget_peer = peers.len() == 0;
    Ok(forget_peer)
}

async fn handle_cancel (
    server: ServerState,
    req: Request,
    peer: PeerState,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
) -> anyhow::Result<bool> {
    handle_bye(server, req, peer, tx).await
}

async fn handle_info (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
) {
    unimplemented!()
}

async fn handle_invite (
    server: ServerState,
    mut req: Request,
    peer: PeerState,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    transport: Transport,
) -> anyhow::Result<()> {
    let is_busy = server.is_busy.swap(true, Ordering::Relaxed);
    if is_busy {
        let resp = make_general_response(&req, StatusCode::BusyEverywhere)?;
        tx.send((resp, peer.addr)).await?;
        return Ok(());
    }

    let req_body = std::mem::take(&mut req.body);
    let call_id = req.call_id_header()?;
    let cseq = req.cseq_header()?;
    let from = req.from_header()?;
    let to = req.to_header()?;
    let via = req.via_header()?;
    let mut is_linphone = false;
    let content_type = req.headers.iter().find_map(|h| {
        match h {
            Header::ContentType(ct) => Some(ct),
            _ => None,
        }
    });
    match content_type {
        Some(ct) => {
            let ct = ct.typed()?.0;
            match ct {
                MediaType::Sdp(sdpct) => {
                    // Do nothing. This is fine.
                },
                _ => {
                    log::warn!("INVITE Request did not have a suitable Content-Type");
                    let resp = make_general_response(&req, StatusCode::UnsupportedMediaType)?;
                    tx.send((resp, peer.addr)).await?;
                    return Ok(());
                },
            };
        },
        None => {
            log::warn!("INVITE Request did not have a Content-Type header");
            let resp = make_general_response(&req, StatusCode::BadRequest)?;
            tx.send((resp, peer.addr)).await?;
            return Ok(());
        }
    };

    // Validate other headers.
    for h in req.headers.iter() {
        match h {
            Header::ContentDisposition(cd) => {
                if cd.to_string() != String::from("session") {
                    log::warn!("Invalid Content-Disposition for SDP descriptor in INVITE");
                    let resp = make_general_response(&req, StatusCode::BadRequest)?;
                    tx.send((resp, peer.addr)).await?;
                    return Ok(());
                }
            },
            Header::UserAgent(ua) => {
                if ua.value().starts_with("LinphoneAndroid") {
                    is_linphone = true;
                }
            }
            _ => {}
        }
    }

    /* NOTE: 
    The initial Request-URI of the message SHOULD be set to the value of
    the URI in the To field.
    Source: https://datatracker.ietf.org/doc/html/rfc3261#section-8.1.1.1
     */
    if let Ok(uri) = &to.uri() {
        if let Some(auth) = &uri.auth {
            if let Some(local_name) = &server.config.local_name {
                if auth.user != *local_name {
                    log::warn!("Call not meant for this server. No such user {}", auth.user);
                    let resp = make_general_response(&req, StatusCode::NotFound)?;
                    tx.send((resp, peer.addr)).await?;
                    return Ok(());
                }
            }
        }
    }

    // Beyond this point, we are assuming the content type is SDP.
    let req_body = match String::from_utf8(req_body) {
        Ok(s) => s,
        Err(_) => {
            // This is a requirement of sdp_rs, not the protocol itself.
            // But it would be insane to have some other weird encoding.
            log::warn!("Invalid INVITE body: not UTF-8 encoded");
            let resp = make_general_response(&req, StatusCode::BadRequest)?;
            tx.send((resp, peer.addr)).await?;
            return Ok(());
        },
    };
    let mut sdreq = SessionDescription::from_str(req_body.as_str())?;
    if sdreq.version != Version::V0 {
        // Technically, this is also supposed to return the supported media types.
        // But this doesn't make sense here, because our problem is not exactly
        // the Content-Type, but its version.
        log::warn!("Invalid INVITE body: SDP not version 0");
        let resp = make_general_response(&req, StatusCode::UnsupportedMediaType)?;
        tx.send((resp, peer.addr)).await?;
        return Ok(());
    }
    
    // We currently do not support any session scheduling.
    let first_time = sdreq.times.first();
    if sdreq.times.len() != 1
        || first_time.active.start != 0
        || first_time.active.stop != 0
        || first_time.repeat.len() != 0 {
        log::warn!("SDP session scheduling via the t= parameter not supported");
        let resp = make_general_response(&req, StatusCode::NotImplemented)?;
        tx.send((resp, peer.addr)).await?;
        return Ok(());
    }
    if sdreq.media_descriptions.len() == 0 {
        log::warn!("SDP descriptor invalid: zero media sessions");
        let resp = make_general_response(&req, StatusCode::BadRequest)?;
        tx.send((resp, peer.addr)).await?;
        return Ok(());
    }

    let mut reqmedia = std::mem::take(&mut sdreq.media_descriptions);
    let mut drop_guards: Vec<DropGuard> = vec![];
    for m in reqmedia.iter_mut() {
        // FIXME: If sendonly (relative to who?), only open RTCP port.
        let supported: bool = m.media.media == sdp_rs::lines::media::MediaType::Audio
            && m.media.proto == ProtoType::RtpAvp
            && m.media.fmt.split(" ").any(|f| f == "8");
        let attrs = std::mem::take(&mut m.attributes);
        let (rtp_port, rtcp_port, cancel) = if supported {
            // This function name is deceptive. It also listens for RTCP.
            listen_for_rtp(server.clone(), peer.addr).await?
        } else {
            (0, 0, CancellationToken::new())
        };
        drop_guards.push(cancel.drop_guard());
        let media = Media{
            media: m.media.media.to_owned(),
            port: rtp_port,
            num_of_ports: None, // TODO: Is this right?
            proto: ProtoType::RtpAvp,
            fmt: "8".to_string(), // Payload Type 8 = G.711 A-Law (PCMA)
        };
        m.media = media;
        // TODO: Validate no contradicting attributes
        let mut direction: Attribute = Attribute::Sendrecv;
        for attr in attrs.iter() {
            match attr {
                Attribute::Recvonly
                | Attribute::Sendonly
                | Attribute::Sendrecv
                | Attribute::Inactive => direction = attr.to_owned(),
                _ => {},
            }
        }
        m.attributes.push(direction);
        m.attributes.push(Attribute::Other("rtcp".into(), Some(rtcp_port.to_string())));
    }
    let mut new_call = CallState::new(call_id.clone());
    new_call.drop_guards = drop_guards;
    let new_call = Arc::new(Mutex::new(new_call));
    let mut calls = peer.calls.write().await;
    if calls.len() > MAX_CALLS_PER_PEER {
        let resp = make_general_response(&req, StatusCode::Forbidden)?;
        tx.send((resp, peer.addr)).await?;
        return Ok(());
    }
    calls.insert(call_id.to_string(), new_call.clone());
    drop(calls);

    let trying = make_general_response(&req, StatusCode::Trying)?;
    tx.send((trying, peer.addr)).await?;

    /* For some reason, it seems like Linphone simply doesn't work if you return
    a ringing response. I have tested returning this without a Trying, with
    delays, etc. Nothing works and I see nothing wrong with the Ringing response.
    No obvious error appears in the Linphone debug logs.
    
    Reported: https://github.com/BelledonneCommunications/linphone-android/issues/2286
    */
    if !is_linphone {
        let ringing = make_general_response(&req, StatusCode::Ringing)?;
        tx.send((ringing, peer.addr)).await?;
    }

    let my_local_ip = local_ip().unwrap(); // FIXME: Remove this once https://github.com/Televiska/sdp-rs/issues/8 is fixed.
    let sdresp = SessionDescription{
        version: sdp_rs::lines::Version::V0,
        // TODO: Blocked on https://github.com/Televiska/sdp-rs/issues/8
        origin: sdreq.origin.to_owned(), // TODO: This has to be changed to this server, because we modify the SDP answer.
        session_name: sdreq.session_name.to_owned(),
        session_info: sdreq.session_info.to_owned(),
        uri: sdreq.uri.to_owned(),
        emails: sdreq.emails.to_owned(),
        phones: sdreq.phones.to_owned(),
        /* It seems that you MUST return this for clients to work. */
        connection: Some(Connection{
            nettype: sdp_rs::lines::common::Nettype::In,
            addrtype: sdp_rs::lines::common::Addrtype::Ip4,
            connection_address: ConnectionAddress{
                base: my_local_ip, // TODO: Bug: this can be a hostname: https://github.com/Televiska/sdp-rs/issues/8
                numaddr: None,
                ttl: None,
            },
        }),
        bandwidths: vec![],
        times: sdreq.times.to_owned(),
        key: None,
        attributes: vec![], // TODO: Maybe copy from the request?
        media_descriptions: reqmedia,
    };
    let contact = get_contact(&server, peer.addr, transport).await;
    let ok_body = sdresp.to_string().into_bytes();
    let mut ok_headers = Headers::default();
    ok_headers.push(Header::CallId(call_id.to_owned()));
    ok_headers.push(Header::CSeq(cseq.to_owned()));
    ok_headers.push(Header::From(from.to_owned()));
    ok_headers.push(Header::To(to.to_owned()));
    ok_headers.push(Header::ContentLength(ContentLength::new(ok_body.len().to_string().as_str())));
    ok_headers.push(contact.into());
    // This is the correct procedure, as long as you are not forwarding, in which
    // case, you will need to add another Via header.
    ok_headers.push(via.clone().into());
    ok_headers.push(Allow(ALLOWED_METHODS.to_vec()).into());
    ok_headers.push(Supported::from(SUPPORTED_OPTIONS.to_string()).into());
    ok_headers.push(Accept::from(vec![
        MediaType::Sdp(vec![]), // Intentionally no parameters
    ]).into());
    ok_headers.push(ContentType(MediaType::Sdp(vec![])).into());
    ok_headers.push(ContentDisposition{
        display_type: DisplayType::Session,
        display_params: vec![],
    }.into());

    let ok = Response{
        status_code: StatusCode::OK,
        version: rsip::Version::V2,
        headers: ok_headers,
        body: ok_body,
        ..Default::default()
    };
    let cseq = cseq.typed()?;
    let mut call = new_call.lock().await;
    call.status = CallStatus::WaitingForAck(cseq);
    drop(call);
    tx.send((ok.to_string().into_bytes(), peer.addr)).await?;
    /*
    From https://datatracker.ietf.org/doc/html/rfc3261#section-13.3.1.4:
    > The 2xx response is passed to the transport with an
    > interval that starts at T1 seconds and doubles for each
    > retransmission until it reaches T2 seconds

    Quite honestly, something about this code stinks. Not returning from
    handle_invite before handle_ack is already processed seems like a bug
    waiting to happen. But I am going to live with this for now. Sorry.

    89cb0590-6389-4501-a8af-119636bb8916
     */
    let mut timer = DEFAULT_T1_MILLISECONDS;
    while timer < DEFAULT_T2_MILLISECONDS {
        sleep(Duration::from_millis(timer.into())).await;
        let call = new_call.lock().await;
        if !call.status.is_waiting() {
            // If we are not waiting anymore, the call has begun.
            return Ok(());
        }
        drop(call);
        tx.send((ok.to_string().into_bytes(), peer.addr)).await?;
        timer *= 2;
    }
    // If we made it here, we failed to receive an ACK that began the call.
    log::warn!("Did not receive ACK in time to begin call {}", call_id.to_string());
    Ok(())
}

async fn handle_message (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
) {
    unimplemented!()
}

async fn handle_notify (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
) {
    unimplemented!()
}

async fn handle_options (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    transport: Transport,
) -> anyhow::Result<()> {
    // See: https://datatracker.ietf.org/doc/html/rfc3261#section-8.1.1
    // A valid SIP request formulated by a UAC MUST, at a minimum, contain
    // the following header fields: To, From, CSeq, Call-ID, Max-Forwards,
    // and Via; all of these header fields are mandatory in all SIP
    // requests.
    let call_id = req.call_id_header()?;
    let cseq = req.cseq_header()?;
    let from = req.from_header()?;
    let to = req.to_header()?;
    let via = req.via_header()?;
    
    let contact = get_contact(&server, peer_addr, transport).await;
    let mut headers = Headers::default();
    headers.push(Header::CallId(call_id.to_owned()));
    headers.push(Header::CSeq(cseq.to_owned()));
    headers.push(Header::From(from.to_owned()));
    headers.push(Header::To(to.to_owned()));
    headers.push(Header::ContentLength(ContentLength::new("0")));

    // This is the correct procedure, as long as you are not forwarding, in which
    // case, you will need to add another Via header.
    headers.push(via.clone().into());
    headers.push(contact.into());
    headers.push(Allow(ALLOWED_METHODS.to_vec()).into());
    headers.push(Supported::from(SUPPORTED_OPTIONS.to_string()).into());
    headers.push(Accept::from(vec![
        MediaType::Sdp(vec![]), // Intentionally no parameters
    ]).into());
    headers.push(AcceptEncoding::new(ACCEPT_ENCODING).into());
    headers.push(AcceptLanguage::new(ACCEPT_LANGUAGE).into());
    headers.push(Server::new(SERVER).into());
    // TODO: Allow-Events (when subscriptions are supported)

    let is_busy = server.is_busy.swap(true, Ordering::Relaxed);
    if is_busy {
        let resp = make_general_response(&req, StatusCode::BusyEverywhere)?;
        tx.send((resp, peer_addr)).await?;
        return Ok(());
    }

    let res = Response{
        status_code: StatusCode::OK,
        version: rsip::Version::V2,
        headers,
        body: vec![],
        ..Default::default()
    };
    tx.send((res.to_string().into_bytes(), peer_addr)).await?;
    Ok(())
}

async fn handle_prack (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
) {
    unimplemented!()
}

async fn handle_publish (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
) {
    unimplemented!()
}

async fn handle_refer (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
) {
    unimplemented!()
}

async fn handle_register (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
) {
    unimplemented!()
}

async fn handle_subscribe (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
) {
    unimplemented!()
}

async fn handle_update (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
) {
    unimplemented!()
}

async fn invalid_sequence (
    req: &Request,
    peer_addr: SocketAddr,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
) -> anyhow::Result<()> {
    log::warn!("Invalid SIP request sequence from peer {}", peer_addr);
    let resp = make_general_response(&req, StatusCode::BadRequest)?;
    tx.send((resp, peer_addr)).await?;
    return Ok(());
}

async fn unsupported_method (
    req: &Request,
    peer_addr: SocketAddr,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
) -> anyhow::Result<()> {
    log::warn!("Unsupported SIP method {} from peer {}", req.method().to_string(), peer_addr);
    let resp = make_general_response(&req, StatusCode::MethodNotAllowed)?;
    tx.send((resp, peer_addr)).await?;
    return Ok(());
}

fn reset_inactivity_timeout (server: ServerState, peer: &mut PeerState) {
    let addr = peer.addr;
    let h = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(server.config.inactivity_timeout.into())).await;
        let mut peers = server.peers.write().await;
        peers.remove(&addr);
    });
    peer.impending_doom.as_ref().and_then(|doom| -> Option<AbortHandle> {
        doom.abort();
        None
    });
    peer.impending_doom = Some(h.abort_handle());
}

async fn handle_request_and_errors (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    transport: Transport,
) -> anyhow::Result<()> {
    let mut peer = {
        let peers_map = server.peers.read().await;
        peers_map.get(&peer_addr).cloned()
    };
    if let Some(p) = peer.as_mut() {
        reset_inactivity_timeout(server.clone(), p);
    }

    match req.method {
        Method::Ack => {
            if peer.is_none() {
                return invalid_sequence(&req, peer_addr, tx).await;
            }
            let peer = peer.unwrap();
            handle_ack(server, req, peer, tx, transport).await
        },
        Method::Bye => {
            if peer.is_none() {
                return invalid_sequence(&req, peer_addr, tx).await;
            }
            let peer = peer.unwrap();
            let forget_peer = handle_bye(server.clone(), req, peer, tx).await?;
            if forget_peer {
                let mut peers = server.peers.write().await;
                peers.remove(&peer_addr);
            }
            Ok(())
        },
        Method::Cancel => {
            if peer.is_none() {
                return invalid_sequence(&req, peer_addr, tx).await;
            }
            let peer = peer.unwrap();
            let forget_peer = handle_cancel(server.clone(), req, peer, tx).await?;
            if forget_peer {
                let mut peers = server.peers.write().await;
                peers.remove(&peer_addr);
            }
            Ok(())
        },
        Method::Info => {
            if peer.is_none() {
                return invalid_sequence(&req, peer_addr, tx).await;
            }
            // let peer = peer.unwrap();
            unsupported_method(&req, peer_addr, tx).await
        },
        Method::Invite => {
            let peer: PeerState = if peer.is_none() {
                log::info!("New peer {}", peer_addr);
                let mut new_peer = PeerState::new(peer_addr);
                reset_inactivity_timeout(server.clone(), &mut new_peer);
                let mut peers = server.peers.write().await;
                peers.insert(peer_addr, new_peer.to_owned());
                drop(peers);
                new_peer
            } else {
                peer.unwrap()
            };
            handle_invite(server, req, peer, tx, transport).await
        },
        Method::Message => {
            unsupported_method(&req, peer_addr, tx).await
        },
        Method::Notify => {
            if peer.is_none() {
                return invalid_sequence(&req, peer_addr, tx).await;
            }
            // let peer = peer.unwrap();
            unsupported_method(&req, peer_addr, tx).await
        },
        Method::Options => {
            handle_options(server, req, peer_addr, tx, transport).await
        },
        Method::PRack => {
            if peer.is_none() {
                return invalid_sequence(&req, peer_addr, tx).await;
            }
            // let peer = peer.unwrap();
            unsupported_method(&req, peer_addr, tx).await
        },
        Method::Publish => {
            unsupported_method(&req, peer_addr, tx).await
        },
        Method::Refer => {
            if peer.is_none() {
                return invalid_sequence(&req, peer_addr, tx).await;
            }
            // let peer = peer.unwrap();
            unsupported_method(&req, peer_addr, tx).await
        },
        Method::Register => {
            unsupported_method(&req, peer_addr, tx).await
        },
        Method::Subscribe => {
            unsupported_method(&req, peer_addr, tx).await
        },
        Method::Update => {
            if peer.is_none() {
                return invalid_sequence(&req, peer_addr, tx).await;
            }
            // let peer = peer.unwrap();
            unsupported_method(&req, peer_addr, tx).await
        }
    }
}

async fn handle_request (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    transport: Transport,
) {
    match handle_request_and_errors(server, req, peer_addr, tx, transport).await {
        Ok(_) => {},
        Err(e) => {
            log::error!("{}", e);
        },
    };
}

async fn get_config () -> anyhow::Result<(Config, Option<PathBuf>)> {
    for possible_path in CONFIG_LOCATIONS.iter() {
        for possible_suffix in CONFIG_SUFFIXES.iter() {
            let path_str = [ *possible_path, *possible_suffix ].join("");
            let path = PathBuf::from_str(path_str.as_str())
                .expect("failed to construct configuration file path");
            match read_to_string(&path).await {
                Ok(s) => return Ok((toml::from_str(&s)?, Some(path))),
                Err(e) => {
                    if e.kind() == ErrorKind::NotFound {
                        continue;
                    }
                }
            };
        }
    }
    Ok((Config::default(), None))
}

#[cfg(not(target_os = "wasi"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::process::exit;

    let mut udp_buf = [0; 65536];

    let (config, config_path) = get_config().await?;
    log4rs::init_config(get_default_log4rs_config()).unwrap();
    if let Some(cp) = config_path {
        log::info!("Using configuration file at {}", cp.display());
    } else {
        log::info!("No configuration file found. Using all defaults.");
    }

    let udp_local_addr = config.sip_udp_address();
    let tcp_local_addr = config.sip_tcp_address();

    let state = ServerState{
        peers: Arc::new(RwLock::new(HashMap::new())),
        is_busy: Arc::new(AtomicBool::new(false)),
        config: Arc::new(config),
    };

    let udp_receive_socket = Arc::new(UdpSocket::bind(udp_local_addr).await.expect("Failed to bind with UDP"));
    let udp_send_socket = udp_receive_socket.clone();
    log::info!("Listening on {}", udp_local_addr);

    let tcp_listener = TcpListener::bind(tcp_local_addr).await.expect("Failed to bind with TCP");
    log::info!("Listening on {}", tcp_local_addr);

    let (tx, mut rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(1_000);
    tokio::spawn(async move {
        while let Some((bytes, addr)) = rx.recv().await {
            udp_send_socket.send_to(&bytes, &addr).await.unwrap(); // TODO: Handle errors
        }
    });

    loop {
        tokio::select! {
            result = udp_receive_socket.recv_from(&mut udp_buf) => {
                if result.is_err() {
                    let e = result.err().unwrap();
                    if e.kind() == ErrorKind::Interrupted {
                        continue;
                    }
                    log::error!("Fatal error reading datagrams from UDP socket: {}", e);
                    log::error!("Shutting down because of the above error.");
                    exit(2);
                }
                let (len, peer_addr) = result.unwrap();

                // For toleration of Linphone's keepalives over UDP, which is
                // INVALID. See: https://lists.gnu.org/archive/html/linphone-developers/2017-06/msg00045.html
                if len == 4 && udp_buf.starts_with([ b'\r', b'\n', b'\r', b'\n' ].as_slice()) {
                    continue;
                }
                let req = match Request::try_from(&udp_buf[0..len]) {
                    Ok(r) => r,
                    Err(e) => {
                        log::error!("Failed to decode SIP message: {}", e);
                        continue;
                    },
                };
                // If you do not spawn this task, each request will have to
                // complete in order. Search 89cb0590-6389-4501-a8af-119636bb8916
                // for a code location where this was a problem.
                tokio::spawn(handle_request(state.to_owned(), req, peer_addr, tx.clone(), Transport::Udp));
            }
            result = tcp_listener.accept() => {
                if result.is_err() {
                    todo!()
                }
                let (tcp_socket, _) = result.unwrap();
                // process_socket(tcp_socket).await;
                // unimplemented!()
                // TODO:
            }
        }
    };
    
    Ok(())
}

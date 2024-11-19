mod logging;
mod rtcp;
mod types;
use crate::logging::get_default_log4rs_config;
use ringbuf::SharedRb;
use rsip::headers::{ContentLength, Supported};
use rsip::prelude::{HasHeaders, HeadersExt, ToTypedHeader, UntypedHeader};
use rsip::typed::content_disposition::DisplayType;
use rtcp::{RTCPHeader, SenderReportHeader};
use sdp_rs::lines::connection::ConnectionAddress;
use sdp_rs::lines::media::ProtoType;
use sdp_rs::lines::{Attribute, Connection, Media};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use types::{CallStatus, RTPStreamState};
use core::str;
use std::net::{SocketAddr, SocketAddrV4, Ipv4Addr};
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
    Header, Headers, Host, Method, Port, Request, Response, StatusCode, Transport, Version
};
use rsip::typed::{Accept, Allow, CSeq, Contact, ContentDisposition, ContentType, MediaType, Via};
use local_ip_address::local_ip;
use sdp_rs::{MediaDescription, SessionDescription};



const udp_server_addr: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 50051));
const tcp_server_addr: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 50052));

// async fn handle_setup (
//     tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
//     peer_addr: SocketAddr,
//     peer: PeerState,
//     setup: callsig::Setup,
//     call_id: Uuid,
//     message_id: u32,
// ) -> anyhow::Result<()> {
//     tx.send((generate_call_proceeding(1), peer_addr)).await?;
//     tx.send((generate_alerting(2), peer_addr)).await?;

//     println!("Accept call from udp://{} ?", peer_addr);
//     if setup.caller_name.len() > 0 {
//         println!("The caller has self-identified as \"{}\".", setup.caller_name);
//     }
//     println!("Press y to accept or n to reject and press ENTER.");
//     let choice: char = tokio::io::stdin().read_u8().await
//         .map(|b| b.into())
//         .unwrap_or('\0');
//     match choice {
//         'y' | 'Y' => {
//             println!("Accepting the call from udp://{},", peer_addr);
//             tx.send((generate_connect(3), peer_addr)).await?;
//         },
//         'n' | 'N' => {
//             println!("Rejecting the call from udp://{}.", peer_addr);
//             let r = generate_release_complete(3, callsig::ReleaseCompleteReason::ReleaseCompleteDestinationRejection);
//             tx.send((r, peer_addr)).await?;
//         },
//         _ => {
//             println!("Error reading user input. Rejecting the call from udp://{}.", peer_addr);
//             let r = generate_release_complete(3, callsig::ReleaseCompleteReason::ReleaseCompleteUndefinedReason);
//             tx.send((r, peer_addr)).await?;
//         }
//     };
//     let call = CallState{
//         id: call_id,
//         capabilities: setup.capabilities.clone(),
//         channels: HashMap::new(),
//         expected_message_id: Arc::new(AtomicU32::new(message_id)),
//         forward_to_ip: None,
//         local_is_master: true,
//     };
//     let mut calls_map = peer.calls.write().await;
//     calls_map.insert(call_id, Arc::new(Mutex::new(call)));
//     Ok(())
// }

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
) {
    let mut rtcp_buf = [0; 65536];
    let mut rtp_buf = [0; 65536];

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                break;
            }
            val = socket.recv_from(&mut rtp_buf) => {
                let (len, peer_addr) = val.unwrap(); // TODO: Handle
                if peer_addr.ip() != correct_peer_addr.ip() {
                    println!("unauthorized RTP packet from {}, but owner is {}", peer_addr, correct_peer_addr);
                    continue; // Just ignore packets from naughtybois.
                }
                match rtp_rs::RtpReader::new(&rtp_buf[0..len]) {
                    Ok(rtp_packet) => {
                        // TODO: Check the stream number and such from the packet.
                        // handle_rtp_packet(&rtp_packet);
                        producer.push_slice(rtp_packet.payload());
                    },
                    Err(e) => {
                        println!("Error decoding RTP packet: {:?}", e); // TODO: Handle this some other way.
                    },
                };
            },
            val = rtcp_socket.recv_from(&mut rtcp_buf) => {
                let (len, peer_addr) = val.unwrap(); // TODO: Handle
                if peer_addr.ip() != correct_peer_addr.ip() {
                    println!("unauthorized RTP packet from {}, but owner is {}", peer_addr, correct_peer_addr);
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
                if rtcp_buf[0] & 0b1100_0000 != 0b1000_0000 {
                    log::warn!("Unsupported RTCP packet version from peer {}", peer_addr);
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

async fn listen_for_rtp (peer_addr: SocketAddr) -> anyhow::Result<(u16, u16, CancellationToken)> {
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
        println!("Listening for RTCP traffic on {}", socket_addr);
        rtcp_socket = socket;
        let socket_addr = SocketAddr::new(ip.into(), assigned_port - 1);
        let rtp_sock = UdpSocket::bind(socket_addr).await?;
        println!("Listening for RTP traffic on {}", socket_addr);
        rtp_sock
    } else {
        println!("Listening for RTP traffic on {}", socket_addr);
        let socket_addr = SocketAddr::new(ip.into(), assigned_port + 1);
        rtcp_socket = UdpSocket::bind(socket_addr).await?;
        println!("Listening for RTCP traffic on {}", socket_addr);
        socket
    };

    let rtp_port = rtp_socket.local_addr()?.port();
    let rtcp_port = rtcp_socket.local_addr()?.port();
    
    let token = CancellationToken::new();

    let ring = HeapRb::<u8>::new(24000);
    let (mut producer, mut consumer) = ring.split();

    let host = cpal::default_host();
    let device = host.default_output_device().expect("No output audio device found");
    let mut supported_configs = device.supported_output_configs()
        .expect("error while querying configs");
    let config = supported_configs
        .find(|c| c.channels() == 1 && c.sample_format() == SampleFormat::I16);
    let config = config.unwrap().with_sample_rate(SampleRate(8000)).into();
    tokio::spawn(receive_rtp_packets(
        peer_addr,
        rtp_socket,
        rtcp_socket,
        token.clone(),
        producer,
    ));

    /* If this approach seems abstruse, let me assure you I did not want to do
    it this way. See: https://github.com/RustAudio/cpal/issues/818 */
    let flag = Arc::new(AtomicBool::new(false));
    let flag2 = Arc::clone(&flag);
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
                    eprintln!("input stream fell behind: try increasing latency");
                }
            },
            move |err| {
                println!("Audio output error: {}", err);
            },
            Some(Duration::from_millis(30)) // None=blocking, Some(Duration)=timeout
        ).expect("Could not build output stream");
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
    });

    Ok((rtp_port, rtcp_port, token))
}

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
) -> anyhow::Result<()> {
    let call_id = req.call_id_header()?;
    let mut calls = peer.calls.write().await;
    let maybe_call = calls.remove(&call_id.to_string());
    if maybe_call.is_none() {
        log::warn!("No such call with identifier {}", call_id.to_string());
        // TODO: Send error response.
    };
    /* In theory, nothing else should be needed to drop the resources used by
    the call. We use DropGuards in CallState so that the RTP sockets get closed
    when the call is ended. */
    Ok(())
}

async fn handle_cancel (
    server: ServerState,
    req: Request,
    peer: PeerState,
) -> anyhow::Result<()> {
    handle_bye(server, req, peer).await
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
    let req_body = std::mem::take(&mut req.body);
    let call_id = req.call_id_header()?;
    let cseq = req.cseq_header()?;
    let from = req.from_header()?;
    let to = req.to_header()?;
    let via = req.via_header()?;
    let content_type = req.headers.iter().find_map(|h| {
        match h {
            Header::ContentType(ct) => Some(ct),
            _ => None,
        }
    });
    match content_type {
        Some(ct) => {
            let ct = ct.typed().unwrap().0; // TODO: Handle error?
            match ct {
                MediaType::Sdp(sdpct) => {
                    // Do nothing. This is fine.
                },
                _ => {
                    // TODO: Respond with error.
                },
            };
        },
        None => {
            // TODO: Respond with error.
        }
    };
    // Beyond this point, we are assuming the content type is SDP.
    // TODO: Ensure that Content-Disposition is either missing or session.
    let req_body = match String::from_utf8(req_body) {
        Ok(s) => s,
        Err(_) => {
            // This is a requirement of sdp_rs, not the protocol itself.
            // But it would be insane to have some other weird encoding.
            log::warn!("Invalid INVITE body: not UTF-8 encoded");
            // TODO: Send error response
            return Ok(());
        },
    };
    let mut sdreq = SessionDescription::from_str(req_body.as_str())?;
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
            listen_for_rtp(peer.addr).await?
        } else {
            (0, 0, CancellationToken::new())
        };
        drop_guards.push(cancel.drop_guard());
        let media = Media{
            media: m.media.media.to_owned(),
            port: rtp_port,
            num_of_ports: None, // TODO: Is this right?
            proto: ProtoType::RtpAvp,
            // TODO: Actually check that 8 was among the options.
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
    // TODO: Ensure its V0
    // TODO: Reject if NOT t=0 0 (we will implement session scheduling later)
    // TODO: Reject if there are zero medias
    // TODO: Select media types and respond.
    let mut new_call = CallState::new(call_id.clone());
    new_call.drop_guards = drop_guards;
    let new_call = Arc::new(Mutex::new(new_call));
    let mut calls = peer.calls.write().await;
    calls.insert(call_id.to_string(), new_call.clone());
    drop(calls);

    // TODO: Check if directed to this server
    // TODO: Write Trying

    // TODO: Via header
    // Via: SIP/2.0/UDP 192.168.1.2;received=80.230.219.70;rport=5060;branch=z9hG4bKnp112903503-43a64480192.168.1.2
    // let via = Via::from("");
    let mut trying_headers = Headers::default();
    trying_headers.push(Header::CallId(call_id.to_owned()));
    trying_headers.push(Header::CSeq(cseq.to_owned()));
    trying_headers.push(Header::From(from.to_owned()));
    trying_headers.push(Header::To(to.to_owned()));
    trying_headers.push(Header::ContentLength(ContentLength::new("0")));
    trying_headers.push(via.clone().into()); // TODO: I don't think this is right.
    let trying = Response{
        status_code: StatusCode::Trying,
        version: Version::V2,
        headers: trying_headers,
        body: vec![],
        ..Default::default()
    };
    tx.send((trying.to_string().into_bytes(), peer.addr)).await?;
    // TODO: Write Ringing

    let my_local_ip = local_ip().unwrap(); // TODO: Handle errors
    let my_host = Host::IpAddr(my_local_ip);
    let base_uri = rsip::Uri {
        scheme: Some(rsip::Scheme::Sip),
        auth: Some(("bob", Option::<String>::None).into()),
        host_with_port: rsip::HostWithPort::from((my_host, 50051)), // FIXME: Configurable port
        ..Default::default()
    };

    let sdresp = SessionDescription{
        version: sdp_rs::lines::Version::V0,
        origin: sdreq.origin.to_owned(), // TODO: This has to be changed to this server, because we modify the SDP answer.
        session_name: sdreq.session_name.to_owned(),
        session_info: sdreq.session_info.to_owned(),
        uri: sdreq.uri.to_owned(),
        emails: sdreq.emails.to_owned(),
        phones: sdreq.phones.to_owned(),
        /* It seems that you MUST return this for clients to work. */
        connection: Some(Connection{
            addrtype: sdp_rs::lines::common::Addrtype::Ip4,
            connection_address: ConnectionAddress{
                base: my_local_ip,
                numaddr: None,
                ttl: None,
            },
            nettype: sdp_rs::lines::common::Nettype::In,
        }),
        bandwidths: vec![],
        times: sdreq.times.to_owned(),
        key: None,
        attributes: vec![], // TODO: Maybe copy from the request?
        media_descriptions: reqmedia,
    };
    let ok_body = sdresp.to_string().into_bytes();
    let mut ok_headers = Headers::default();
    ok_headers.push(Header::CallId(call_id.to_owned()));
    ok_headers.push(Header::CSeq(cseq.to_owned()));
    ok_headers.push(Header::From(from.to_owned()));
    ok_headers.push(Header::To(to.to_owned()));
    ok_headers.push(Header::ContentLength(ContentLength::new(ok_body.len().to_string().as_str())));
    ok_headers.push(Contact{
        display_name: Some("skwisgar".into()),
        uri: base_uri.clone(),
        params: vec![rsip::Param::Branch(rsip::param::Branch::new(
            "z9hG4bKqoetijoqijiq", // TODO: Get this from the request.
        ))],
    }.into());
    ok_headers.push(via.clone().into()); // TODO: I don't think this is right.
    ok_headers.push(Allow(vec![ Method::Invite, Method::Ack, Method::Bye, Method::Cancel, Method::Options ]).into());
    ok_headers.push(Supported::from("".to_string()).into());
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
        version: Version::V2,
        headers: ok_headers,
        body: ok_body,
        ..Default::default()
    };
    let mut call = new_call.lock().await;
    call.status = CallStatus::WaitingForAck(cseq.typed().unwrap()); // TODO: Handle this error? What could even happen?
    tx.send((ok.to_string().into_bytes(), peer.addr)).await?;
    // TODO: Implement re-transmission described in https://datatracker.ietf.org/doc/html/rfc3261#section-13.3.1.4
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
) {
    unimplemented!()
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

async fn invalid_sequence (peer_addr: SocketAddr) {
    unimplemented!()
}

async fn handle_request_and_errors (
    server: ServerState,
    req: Request,
    peer_addr: SocketAddr,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    transport: Transport,
) -> anyhow::Result<()> {
    let peer = {
        let peers_map = server.peers.read().await;
        peers_map.get(&peer_addr).cloned()
    };
    match req.method {
        Method::Ack => {
            if peer.is_none() {
                invalid_sequence(peer_addr).await;
                return Ok(());
            }
            let peer = peer.unwrap();
            handle_ack(server, req, peer, tx, transport).await
        },
        Method::Bye => {
            if peer.is_none() {
                invalid_sequence(peer_addr).await;
                return Ok(());
            }
            let peer = peer.unwrap();
            handle_bye(server, req, peer).await
        },
        Method::Cancel => {
            if peer.is_none() {
                invalid_sequence(peer_addr).await;
                return Ok(());
            }
            let peer = peer.unwrap();
            handle_cancel(server, req, peer).await
        },
        Method::Info => {
            if peer.is_none() {
                invalid_sequence(peer_addr).await;
                return Ok(());
            }
            let peer = peer.unwrap();
            unimplemented!()
        },
        Method::Invite => {
            let peer: PeerState = if peer.is_none() {
                log::info!("New peer {}", peer_addr);
                let new_peer = PeerState{
                    addr: peer_addr,
                    calls: Arc::new(RwLock::new(HashMap::new())),
                };
                let mut peers = server.peers.write().await;
                peers.insert(peer_addr, new_peer.to_owned());
                new_peer
            } else {
                peer.unwrap()
            };
            handle_invite(server, req, peer, tx, transport).await
        },
        Method::Message => {
            unimplemented!()
        },
        Method::Notify => {
            if peer.is_none() {
                invalid_sequence(peer_addr).await;
                return Ok(());
            }
            let peer = peer.unwrap();
            unimplemented!()
        },
        Method::Options => {
            unimplemented!()
        },
        Method::PRack => {
            if peer.is_none() {
                invalid_sequence(peer_addr).await;
                return Ok(());
            }
            let peer = peer.unwrap();
            unimplemented!()
        },
        Method::Publish => {
            unimplemented!()
        },
        Method::Refer => {
            if peer.is_none() {
                invalid_sequence(peer_addr).await;
                return Ok(());
            }
            let peer = peer.unwrap();
            unimplemented!()
        },
        Method::Register => {
            unimplemented!()
        },
        Method::Subscribe => {
            unimplemented!()
        },
        Method::Update => {
            if peer.is_none() {
                invalid_sequence(peer_addr).await;
                return Ok(());
            }
            let peer = peer.unwrap();
            unimplemented!()
        },
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

#[cfg(not(target_os = "wasi"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    let mut udp_buf = [0; 65536];
    let state = ServerState{
        peers: Arc::new(RwLock::new(HashMap::new())),
    };

    log4rs::init_config(get_default_log4rs_config()).unwrap();
    let udp_local_addr: SocketAddr = udp_server_addr;
    let tcp_local_addr: SocketAddr = tcp_server_addr;

    let udp_receive_socket = Arc::new(UdpSocket::bind(udp_local_addr).await.expect("Failed to bind with UDP"));
    let udp_send_socket = udp_receive_socket.clone();
    log::info!("Listening on {}", udp_local_addr);

    let tcp_listener = TcpListener::bind(tcp_local_addr).await.expect("Failed to bind with TCP");
    log::info!("Listening on {}", tcp_local_addr);

    let (tx, mut rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(1_000);
    tokio::spawn(async move {
        while let Some((bytes, addr)) = rx.recv().await {
            let len = udp_send_socket.send_to(&bytes, &addr).await.unwrap();
            println!("{:?} bytes sent", len);
        }
    });

    loop {
        tokio::select! {
            result = udp_receive_socket.recv_from(&mut udp_buf) => {
                if result.is_err() {
                    todo!()
                }
                let (len, peer_addr) = result.unwrap();
                let req = match Request::try_from(&udp_buf[0..len]) {
                    Ok(r) => r,
                    Err(e) => {
                        log::error!("Failed to decode SIP message: {}", e);
                        continue;
                    },
                };
                handle_request(state.to_owned(), req, peer_addr, tx.clone(), Transport::Udp).await;
            }
            result = tcp_listener.accept() => {
                if result.is_err() {
                    todo!()
                }
                let (tcp_socket, _) = result.unwrap();
                // process_socket(tcp_socket).await;
                unimplemented!()
            }
        }
    };
    
    Ok(())
}

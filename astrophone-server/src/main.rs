mod p;
mod logging;
mod rtcp;
mod types;
use crate::logging::get_default_log4rs_config;
use ringbuf::SharedRb;
use rtcp::{RTCPHeader, SenderReportHeader};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use types::RTPStreamState;
use core::str;
use std::net::{SocketAddr, SocketAddrV4, Ipv4Addr};
use std::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use std::time::{Duration, Instant};
use prost::{bytes::BytesMut, Message};
use anyhow;
use std::collections::HashMap;
use uuid::Uuid;
use tokio::sync::{Mutex, mpsc};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use p::callsig;
use p::mediacontrol;
use tokio_util::sync::CancellationToken;
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

const server_addr: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 50051));

fn generate_err_message (
    message_id: u32,
    code: callsig::CallSignallingErrorCode,
    message: Option<String>
) -> Vec<u8> {
    let e = callsig::CallSignallingError{
        message_id,
        code: code.into(),
        message: message.unwrap_or_default(),
        ..Default::default()
    };
    let cs = callsig::CallSignalling{
        message_id,
        call_id: None,
        body: Some(callsig::call_signalling::Body::Error(e)),
        media_control: vec![],
        call_linkage: None,
    };
    cs.encode_to_vec()
}

fn generate_call_proceeding (message_id: u32) -> Vec<u8> {
    let m = callsig::CallProceeding{};
    let cs = callsig::CallSignalling{
        message_id,
        call_id: None,
        body: Some(callsig::call_signalling::Body::CallProceeding(m)),
        media_control: vec![],
        call_linkage: None,
    };
    cs.encode_to_vec()
}

fn generate_alerting (message_id: u32) -> Vec<u8> {
    let m = callsig::Alerting{
        ..Default::default()
    };
    let cs = callsig::CallSignalling{
        message_id,
        call_id: None,
        body: Some(callsig::call_signalling::Body::Alerting(m)),
        media_control: vec![],
        call_linkage: None,
    };
    cs.encode_to_vec()
}

fn generate_connect (message_id: u32) -> Vec<u8> {
    let m = callsig::Connect{
        // TODO: Include capabilities once you get this working.
        capabilities: Some(mediacontrol::TerminalCapabilitySet{
            sequence_number: 1,
            protocol_identifier: vec![],
            ..Default::default()
        }),
        ..Default::default()
    };
    let cs = callsig::CallSignalling{
        message_id,
        call_id: None,
        body: Some(callsig::call_signalling::Body::Connect(m)),
        media_control: vec![],
        call_linkage: None,
    };
    cs.encode_to_vec()
}

fn generate_olc_ack (message_id: u32, fwcn: u32, rtp_port: u16, rtcp_port: u16) -> Vec<u8> {
    let ip = mediacontrol::IpAddress { version: Some(mediacontrol::ip_address::Version::V4([ 127, 0, 0, 1 ].into())) };
    let params = mediacontrol::H2250LogicalChannelAckParameters{
        port_number: Some(rtp_port.into()),
        media_channel: Some(mediacontrol::TransportAddress{
            variant: Some(mediacontrol::transport_address::Variant::IpAddress(ip.to_owned())),
            port: rtp_port.into(),
            ..Default::default()
        }),
        media_control_channel: Some(mediacontrol::TransportAddress{
            variant: Some(mediacontrol::transport_address::Variant::IpAddress(ip)),
            port: rtcp_port.into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    // let reverse = mediacontrol::ReverseLogicalChannelParameters{
    //     logical_channel_number: 1, // Does this have to be unique among the FW channel numbers? Assign these in a better way
    //     ..Default::default()
    // };
    let olc_ack = mediacontrol::OpenLogicalChannelAck{
        forward_logical_channel_number: fwcn,
        h2250_logical_channel_ack_parameters: Some(params),
        reverse: None,
        ..Default::default()
    };
    let m = mediacontrol::MediaControlMessage{
        message_id, // TODO: Just make this automatically inferred from the CS message ID.
        variant: Some(mediacontrol::media_control_message::Variant::OpenLogicalChannelAck(olc_ack)),
        ..Default::default()
    };
    let cs = callsig::CallSignalling{
        message_id,
        media_control: vec![ m ],
        ..Default::default()
    };
    cs.encode_to_vec()
}

fn generate_release_complete (
    message_id: u32,
    reason: callsig::ReleaseCompleteReason,
) -> Vec<u8> {
    let m = callsig::ReleaseComplete{
        reason: reason.into(),
        ..Default::default()
    };
    let cs = callsig::CallSignalling{
        message_id,
        call_id: None,
        body: Some(callsig::call_signalling::Body::ReleaseComplete(m)),
        media_control: vec![],
        call_linkage: None,
    };
    cs.encode_to_vec()
}

async fn handle_setup (
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    peer_addr: SocketAddr,
    peer: PeerState,
    setup: callsig::Setup,
    call_id: Uuid,
    message_id: u32,
) -> anyhow::Result<()> {
    tx.send((generate_call_proceeding(1), peer_addr)).await?;
    tx.send((generate_alerting(2), peer_addr)).await?;

    println!("Accept call from udp://{} ?", peer_addr);
    if setup.caller_name.len() > 0 {
        println!("The caller has self-identified as \"{}\".", setup.caller_name);
    }
    println!("Press y to accept or n to reject and press ENTER.");
    let choice: char = tokio::io::stdin().read_u8().await
        .map(|b| b.into())
        .unwrap_or('\0');
    match choice {
        'y' | 'Y' => {
            println!("Accepting the call from udp://{},", peer_addr);
            tx.send((generate_connect(3), peer_addr)).await?;
        },
        'n' | 'N' => {
            println!("Rejecting the call from udp://{}.", peer_addr);
            let r = generate_release_complete(3, callsig::ReleaseCompleteReason::ReleaseCompleteDestinationRejection);
            tx.send((r, peer_addr)).await?;
        },
        _ => {
            println!("Error reading user input. Rejecting the call from udp://{}.", peer_addr);
            let r = generate_release_complete(3, callsig::ReleaseCompleteReason::ReleaseCompleteUndefinedReason);
            tx.send((r, peer_addr)).await?;
        }
    };
    let call = CallState{
        id: call_id,
        capabilities: setup.capabilities.clone(),
        channels: HashMap::new(),
        expected_message_id: Arc::new(AtomicU32::new(message_id)),
        forward_to_ip: None,
        local_is_master: true,
    };
    let mut calls_map = peer.calls.write().await;
    calls_map.insert(call_id, Arc::new(Mutex::new(call)));
    Ok(())
}


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
    let ip = Ipv4Addr::new(127, 0, 0, 1); // TODO: Make this configurable.
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

    let ring = HeapRb::<u8>::new(1200);
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

async fn handle_open_logical_channel (
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    peer_addr: SocketAddr,
    call: Arc<Mutex<CallState>>,
    message_id: u32,
    mc: mediacontrol::OpenLogicalChannel,
) {
    if mc.forward.is_none() {
        println!("Missing forward parameters from OLC from peer {}.", peer_addr);
        // TODO: Return error
        return;
    }
    if mc.separate_stack.is_some() {
        println!("Use of separateStack from peer {} is unsupported.", peer_addr);
        // TODO: Return error (unsupported)
        return;
    }
    if mc.encryption_sync.is_some() {
        println!("Use of encryptionSync from peer {} is unsupported.", peer_addr);
        // TODO: Return error (unsupported)
        return;
    }

    let fcn = mc.forward_logical_channel_number;
    let mediacontrol::OpenLogicalChannel {
        forward,
        reverse,
        ..
    } = mc.clone();
    if reverse.is_some() {
        println!("Reverse logical channel not supported in OLC from peer {}.", peer_addr);
        return;
    }
    let forward = forward.unwrap();
    let flcd = forward.forward_logical_channel_dependency;
    if forward.data_type.is_none() {
        println!("Missing dataType field in forward OLC parameters from peer {}", peer_addr);
        // TODO: Return error
        return;
    }
    let forward_data_type = forward.data_type.unwrap();
    if forward_data_type.variant.is_none() {
        println!("Missing dataType field in forward OLC parameters from peer {}", peer_addr);
        // TODO: Return error
        return;
    }
    let forward_data_type = forward_data_type.variant.unwrap();
    // if forward.multiplex_parameters.is_none() {
    //     println!("Missing multiplexParameters in forward OLC from peer {}.", peer_addr);
    //     return;
    // }
    // let fwdmux = forward.multiplex_parameters.unwrap();
    // // TODO: Actually use this.

    // if fwdmux.media_guaranteed_delivery() || fwdmux.media_control_guaranteed_delivery() {
    //     println!("Cannot guaranteed media or media control delivery from peer {}.", peer_addr);
    //     return;
    // }
    // if fwdmux.redundancy_encoding.is_some() {
    //     println!("Cannot handle redundancy encoding from peer {}.", peer_addr);
    //     return;
    // }
    // TODO: I am not sure what silenceSuppression should be.
    // TODO: Validate mediaPacketization.

    let mut call = call.lock().await;
    if call.channels.contains_key(&fcn) {
        // TODO: Return error (dup channel ID)
        println!("Duplicate forward logical channel number {} from peer {}.", fcn, peer_addr);
        return;
    }
    if flcd > 0 && !call.channels.contains_key(&flcd) {
        println!("Dependency on channel {} not met for OLC from peer {}.",
            forward.forward_logical_channel_dependency, peer_addr);
        // TODO: Return error (dependency not met)
        return;
    }
    let new_channel: ChannelState;
    match forward_data_type {
        mediacontrol::data_type::Variant::Audio(audio) => {
            if audio.variant.is_none() {
                println!("Malformed audio variant from peer {}.", peer_addr);
                return;
            }
            match audio.variant.unwrap() {
                mediacontrol::audio_capability::Variant::G711Alaw56k(fpp) => {
                    // It seems like the called party gets to assign the RTP port.
                    let (rtp_port, rtcp_port, canceller) = listen_for_rtp(peer_addr).await.unwrap(); // TODO: Handle error
                    new_channel = ChannelState{
                        olc: mc,
                        recv: RTPStreamState{
                            expected_sequence_number: 0,
                            initial_ssrc: 0,
                            initial_timestamp: 0,
                            stream_start_time: Instant::now(),
                            canceller,
                        },
                        forward_to_port: None,
                    };
                    tx.send((generate_olc_ack(6, fcn, rtp_port, rtcp_port), peer_addr)).await;
                },
                _ => {
                    // TODO: Return error (data type not supported)
                    println!("Unsupported audio data type from peer {}.", peer_addr);
                    return;
                }
            };
        },
        _ => {
            // TODO: Return error (data type not supported)
            println!("Unsupported non-audio data type from peer {}.", peer_addr);
            return;
        },
    };
    call.channels.insert(fcn, new_channel);
    if forward.replacement_for > 0 {
        call.channels.remove(&forward.replacement_for);
    }
    // I think portNumber is actually always going to be 0. I could be wrong.
}

async fn handle_media_control (
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    peer_addr: SocketAddr,
    call: Arc<Mutex<CallState>>,
    mc: mediacontrol::MediaControlMessage,
) {
    if mc.variant.is_none() {
        let response = generate_err_message(0, callsig::CallSignallingErrorCode::CsErrProceduralError, None);
        /* We intentionally do not handle failure to respond. It's okay if
        we fail to respond to clients that are sending invalid packets. */
        tx.send((response, peer_addr)).await;
        return;
    }
    let message_id = mc.message_id;
    let mc = mc.variant.unwrap();
    match mc {
        p::mediacontrol::media_control_message::Variant::OpenLogicalChannel(m)
            => {
                handle_open_logical_channel(tx, peer_addr, call, message_id, m).await;
            },
        p::mediacontrol::media_control_message::Variant::CloseLogicalChannel(m)
            => {
                todo!()
            },
        p::mediacontrol::media_control_message::Variant::CloseLogicalChannelAck(m)
            => {
                todo!()
            },
        p::mediacontrol::media_control_message::Variant::RequestChannelClose(m)
            => {
                todo!()
            },
        p::mediacontrol::media_control_message::Variant::RequestChannelCloseAck(m)
            => {
                todo!()
            },
        p::mediacontrol::media_control_message::Variant::RequestChannelCloseReject(m)
            => {
                todo!()
            },
        p::mediacontrol::media_control_message::Variant::RequestChannelCloseRelease(m)
            => {
                todo!()
            },
        _ => {

        }
    }
}

async fn handle_packet (
    server: ServerState,
    tx: mpsc::Sender<(Vec<u8>, SocketAddr)>,
    peer_addr: SocketAddr,
    peer: Option<PeerState>,
    cs: callsig::CallSignalling
) {
    if cs.call_id.is_none() {
        todo!("handle error");
    }
    let call_id = cs.call_id.unwrap();
    let call_id = Uuid::try_from(call_id.uuid).unwrap(); // TODO: Handle error
    let call = if peer.is_none() {
        None
    } else {
        let peer = peer.unwrap();
        let call_map = peer.calls.read().await;
        call_map.get(&call_id).cloned()
    };
    if call.is_none() { // If we never heard of this peer before...
        if cs.body.is_none() {
            let response = generate_err_message(0, callsig::CallSignallingErrorCode::CsErrInvalidCallState, None);
            /* We intentionally do not handle failure to respond. It's okay if
            we fail to respond to clients that are sending invalid packets. */
            tx.send((response, peer_addr)).await;
            // TODO: Close the socket whenever there is an error.
            return;
        }
        let body = cs.body.unwrap();
        let peer = PeerState{
            addr: peer_addr,
            calls: Arc::new(RwLock::new(HashMap::new())),
        };
        if let callsig::call_signalling::Body::Setup(setup) = body {
            handle_setup(tx, peer_addr, peer.clone(), setup, call_id, cs.message_id).await; // TODO: Handle errors?
        } else {
            let response = generate_err_message(0, callsig::CallSignallingErrorCode::CsErrInvalidCallState, None);
            /* We intentionally do not handle failure to respond. It's okay if
            we fail to respond to clients that are sending invalid packets. */
            tx.send((response, peer_addr)).await;
        }
        let mut peer_map = server.peers.write().await;
        peer_map.insert(peer_addr, peer);
        return;
    }
    let call = call.unwrap();
    if let Some(body) = &cs.body {
        // TODO: Handle different CS messages
        // TODO: Setup
        // TODO: ReleaseComplete
    }
    for mcm in cs.media_control.into_iter() {
        handle_media_control(tx.clone(), peer_addr, call.clone(), mcm).await;
    }
}

#[cfg(not(target_os = "wasi"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use prost::Message;
    log4rs::init_config(get_default_log4rs_config()).unwrap();
    let local_addr: SocketAddr = server_addr;
    let mut buf = [0; 65536];
    let state = ServerState{
        peers: Arc::new(RwLock::new(HashMap::new())),
    };
    let receive_socket = Arc::new(UdpSocket::bind(local_addr).await.expect("Failed to bind"));
    let send_socket = receive_socket.clone();
    log::info!("Listening on {}", local_addr);

    let (tx, mut rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(1_000);
    tokio::spawn(async move {
        while let Some((bytes, addr)) = rx.recv().await {
            let len = send_socket.send_to(&bytes, &addr).await.unwrap();
            println!("{:?} bytes sent", len);
        }
    });

    loop {
        let (len, peer_addr) = receive_socket.recv_from(&mut buf).await?;
        let msg = BytesMut::from(&buf[0..len]);
        match callsig::CallSignalling::decode(msg) {
            Ok(cs) => {
                let peer = {
                    let peers_map = state.peers.read().await;
                    peers_map.get(&peer_addr).cloned()
                };
                tokio::spawn(handle_packet(state.clone(), tx.clone(), peer_addr, peer, cs));
            },
            Err(e) => {
                log::warn!("Malformed packet from {} error: {}", peer_addr, e);
            }
        }
    }
    
    Ok(())
}

mod p;
mod logging;
mod types;
use crate::logging::get_default_log4rs_config;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use types::RTPStreamState;
use std::net::SocketAddr;
use std::sync::atomic::AtomicU32;
use std::time::Instant;
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

fn generate_olc_ack (message_id: u32, fwcn: u32, port_number: u16) -> Vec<u8> {
    let params = mediacontrol::H2250LogicalChannelAckParameters{
        port_number: Some(port_number.into()),
        ..Default::default()
    };
    let olc_ack = mediacontrol::OpenLogicalChannelAck{
        forward_logical_channel_number: fwcn,
        h2250_logical_channel_ack_parameters: Some(params),
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
    if rtp_packet.version() != 2 { // TODO: Handle version 1?
        return;
    }
    // TODO: If this is the first RTP packet, record the initial timestamp and sequence number.
    // (since the timestamp can be random)
    // TODO: Record the SSRC. Ignore subsequent packets that differ in this regard.
    // TODO: If the RTP packet is due to be played now, play it now, otherwise, schedule it in the future.
}

async fn receive_rtp_packets (
    correct_peer_addr: SocketAddr,
    socket: UdpSocket,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let mut buf = [0; 65536];

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                break;
            }
            val = socket.recv_from(&mut buf) => {
                let (len, peer_addr) = val.unwrap(); // TODO: Handle
                if peer_addr != correct_peer_addr {
                    // TODO: Log or block?
                    break; // Just ignore packets from naughtybois.
                }
                match rtp_rs::RtpReader::new(&buf[0..len]) {
                    Ok(rtp_packet) => {
                        // TODO: Check the stream number and such from the packet.
                        handle_rtp_packet(&rtp_packet);
                    },
                    Err(e) => {
                        println!("Error decoding RTP packet: {:?}", e); // TODO: Handle this some other way.
                    },
                };
            }
        }
    };
    Ok(())
}

async fn listen_for_rtp (peer_addr: SocketAddr) -> anyhow::Result<(u16, CancellationToken)> {
    // TODO: Pick an unused port
    let port = 9099;
    let socket_addr = SocketAddr::new([0,0,0,0].into(), port);
    let socket = UdpSocket::bind(socket_addr).await?;
    println!("Listening for RTP traffic on {}", socket_addr);
    let token = CancellationToken::new();
    tokio::spawn(receive_rtp_packets(peer_addr, socket, token.clone()));
    Ok((port, token))
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
                    let (port, canceller) = listen_for_rtp(peer_addr).await.unwrap(); // TODO: Handle error
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
                    tx.send((generate_olc_ack(6, fcn, port), peer_addr)).await;
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
    let local_addr: SocketAddr = "127.0.0.1:50051".parse()?;
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

/* To be clear, this is not a real program intended for any use right now. This
is just a minimum, crappy, low-effort client just for the sake of manually
testing. */
mod p;
use tokio::net::UdpSocket;
use prost::bytes::BytesMut;
use p::{callsig, mediacontrol};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket as NetUdpSocket};

static mut stage: u32 = 0;

fn send_rtp (to: SocketAddr) {
    let socket = NetUdpSocket::bind("0.0.0.0").expect("Could not bind");
    socket.connect(to).unwrap();
    // socket.send(buf);
}

fn handle_packet (cs: callsig::CallSignalling) -> u32 {
    println!("{:?}", cs);
    // let body = cs.body.unwrap();
    if let Some(body) = cs.body {
        match body {
            p::callsig::call_signalling::Body::Connect(c) => {
                unsafe {
                    stage += 1;
                }
            },
            _ => {
    
            },
        }
    }
    for mc in cs.media_control.iter() {
        match mc.variant.as_ref().unwrap() {
            mediacontrol::media_control_message::Variant::OpenLogicalChannelAck(olc_ack) => {
                let fwdlc_ack = olc_ack.h2250_logical_channel_ack_parameters.as_ref().unwrap();
                let fwd_chan = fwdlc_ack.media_channel.as_ref().unwrap();
                match fwd_chan.variant.as_ref().unwrap() {
                    mediacontrol::transport_address::Variant::IpAddress(ip) => {
                        match ip.version.as_ref().unwrap() {
                            mediacontrol::ip_address::Version::V4(v4) => {
                                let ipv4 = Ipv4Addr::new(v4[0], v4[1], v4[2], v4[3]);
                                let ip = IpAddr::V4(ipv4);
                                let socket_addr = SocketAddr::new(ip, fwd_chan.port as u16);
                            },
                            mediacontrol::ip_address::Version::V6(v6) => {
                                unimplemented!()
                            }
                        }
                    },
                    _ => {
                        unimplemented!()
                    }
                }
            },
            _ => {

            },
        };
    }
    unsafe {
        stage
    }
}

#[cfg(not(target_os = "wasi"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use p::mediacontrol;
    use prost::Message;

    let sock = UdpSocket::bind("0.0.0.0:3337").await?;

    let remote_addr = "127.0.0.1:50051";
    sock.connect(remote_addr).await?;
    let mut buf = [0; 65535];

    let setup = callsig::Setup{
        caller_name: String::from("skwisgar"),
        ..Default::default()
    };
    let setup_cs = callsig::CallSignalling{
        message_id: 1,
        call_id: Some(p::callsig::CallIdentifier{
            uuid: [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16].to_vec(),
            ..Default::default()
        }),
        media_control: vec![],
        call_linkage: None,
        body: Some(callsig::call_signalling::Body::Setup(setup)),
    };
    let len = sock.send(setup_cs.encode_to_vec().as_slice()).await?;
    println!("{:?} bytes sent", len);

    loop {
        let len = sock.recv(&mut buf).await?;
        println!("{:?} bytes received from {:?}", len, remote_addr);
        let mut msg = BytesMut::from(&buf[0..len]);
        match callsig::CallSignalling::decode(msg) {
            Ok(cs) => {
                let s = handle_packet(cs);
                match s {
                    1 => { // CONNECT received: now send OLC
                        let audio_cap = mediacontrol::AudioCapability{
                            variant: Some(mediacontrol::audio_capability::Variant::G711Alaw56k(1)),
                        };
                        let data_type = mediacontrol::DataType{
                            variant: Some(mediacontrol::data_type::Variant::Audio(audio_cap)),
                        };
                        let olc = mediacontrol::OpenLogicalChannel{
                            forward_logical_channel_number: 1,
                            forward: Some(mediacontrol::ForwardLogicalChannelParams{
                                data_type: Some(data_type),
                                ..Default::default()
                            }),
                            ..Default::default()
                        };
                        let mc = mediacontrol::MediaControlMessage{
                            message_id: 2,
                            variant: Some(mediacontrol::media_control_message::Variant::OpenLogicalChannel(olc)),
                        };
                        let olc_cs = callsig::CallSignalling{
                            message_id: 2,
                            call_id: Some(p::callsig::CallIdentifier{
                                uuid: [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16].to_vec(),
                                ..Default::default()
                            }),
                            body: None,
                            call_linkage: None,
                            media_control: vec![mc],
                        };
                        let len = sock.send(olc_cs.encode_to_vec().as_slice()).await?;
                        println!("{:?} bytes sent", len);
                        unsafe {
                            stage += 1;
                        }
                    },
                    _ => {},
                };
            },
            Err(e) => {
                log::warn!("Malformed packet from {}", remote_addr);
            }
        }
    }

    Ok(())
}

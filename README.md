# Astrophone

An incubating VoIP project that is based off of the H.323 protocols, but uses
Protocol Buffers for the wire protocol and reduces the complexity of H.323 by
ignoring anything related to traditional telecom services (PSTN, ISDN, etc.).

## Why?

The H.323 specifications, which are often regarded as "legacy" (meaning, "I'm
too dumb to understand it"), have some salvageable parts. One thing that
interested me was call signalling, in particular. Even in "modern" WebRTC,
there is no signalling protocol. There is no way for somebody to initiate a
WebRTC call to you from an app different than the one they are using. (To
further dunk on WebRTC, it is basically just an abstruse RTP.) I want a
complete, usable protocol, not a half-baked one: I want to be able to use one
application and place a VoIP call to somebody else that might be using another.

Not only do I want this, but I also want all of the call signalling features
described in the H.323 specifications, such as Call Diversion, Hold, Park and
Pickup, Intrustion, Conferencing etc.

Seeing that, even in 2024, there seems to be no specification or protocol for
doing this, I decided to write my own. I know better than to start something
like this from scratch, when I know a lot of real-world wisdom has been
incorporated into the H.323 specifications, so I started with their ASN.1
modules and translated those to their ProtocolBuffers equivalents, after
simplifying some things by making some assumptions about the underlying
transport and such.

## Status

Current, I have a server that receives a Setup message from a new client, and
sends CallProceeding, Alerting, and Connect messages in response, and it
receives an OpenLogicalChannel message and opens an RTP port in response.
It receives the RTP packets from this port, decodes them, and plays them to
your default audio device. Only the G.711 audio "codec" is supported at the
moment.

The "client" is basically just for testing. The code is intentionally the
crappiest and low-effort, because I just want to get the server working, after
which I will perfect the client.

## Version 1.0.0

- [ ] Server
  - [x] `INVITE`
  - [x] `ACK`
    - [ ] What to do with the ports before this is received?
    - [ ] Clean up session if `ACK` not received in time.
  - [ ] `BYE`
  - [ ] `CANCEL`
  - [ ] `OPTIONS`
  - [ ] Clean up dangling dialogs
  - [ ] Proper logging
  - [ ] Configuration or Discovery of Own IP address
  - [ ] Handling of unknown messages
  - [ ] RTCP
  - [ ] G.711 u-Law (PCMU)
- [ ] Client Library
- [ ] CLI / Shell
  - [ ] `call` Subcommand
  - [ ] `help` Subcommand
  - [ ] `hangup` Subcommand
- [ ] Documentation

## Version 1.1.0

- [ ] Bidirectional calls

## Version 1.2.0

- [ ] dTLS Support

## Version 1.3.0

- [ ] Server
  - [ ] Transport over HTTP/3
  - [ ] Multi-User
  - [ ] Relay / Gateway / MITM
  - [ ] NAT Traversal
- [ ] Client
  - [ ] Transport over HTTP/3

I think NAT Traversal is going to basically require me breaking RTP entirely or
abusing its extensions or something. If you have somebody on your network
connecting to a video conference with 100 people on it, they all have their
cameras on, and they all have their microphones on, that's 200 ports you have to
open up on your router to receive all these streams. Then multiply it by the
number of people on your network doing this. Then double it for the control
ports.

Fortunately, one of the nice things about this approach is that you can just
spin up two ports.

And if we're going to break from tradition this much, we might as well use a
Protobuf version of RTCP.

But RTP still seems valuable though. Opinion online reveals that this protocol
is purposely very minimal in the interest of performance, so using protobuf for
a new "RTP" header is out of the question.

I think I am just going to use SSRC for multiplexing. The arguments put forth in
IETF RFC 3550, Section 5.2 are not very compelling.

> UPDATE: It sounds like
> [IETF RFC 8108](https://www.rfc-editor.org/rfc/rfc8108.txt) explicitly allows
> using SSRCs for multiplexing. 

With that established, now I wonder if I should use a Protobuf RTCP or the
original RTCP. RTP is what I would consider to be a perfect protocol (other
than not having clear multiplexing support): it is great for getting real-time
data from one computer to another with the minimal amount of complexity. But
RTCP is a little different: it is not sent frequently, and the packets have a
few more fields and flexibility, so there is a lot more parsing burden.

- Use old RTCP
  - Better compatibility
- Use Protobuf RTCP
  - Easier implementation
  - Use a single port for call signalling, media control, and RTCP.

## Version 1.4.0

- [ ] Server
  - [ ] Conferencing

## Version 1.5.0 and Beyond

- [ ] G.711 u-Law Audio
- [ ] G.711.0 Audio?
- [ ] G.729 Audio?
- [ ] Opus Audio
- [ ] `RequestMode` (`stream` subcommand)
- [ ] `RoundTripDelayRequest`
- [ ] `MaintenanceLoopRequest`
- [ ] `CommunicationModeRequest`
- [ ] `ConferenceRequest`
- [ ] `LogicalChannelRateRequest`
- [ ] `MaintenanceLoopOffCommand`
- [ ] `SendTerminalCapabilitySet`
- [ ] `FlowControlCommand`
- [ ] `EndSessionCommand`
- [ ] `MiscellaneousCommand`
- [ ] `CommunicationModeCommand`
- [ ] `ConferenceCommand`
- [ ] `FunctionNotUnderstood`
- [ ] `TerminalCapabilitySetRelease`
- [ ] `OpenLogicalChannelConfirm`
- [ ] `RequestChannelCloseRelease`
- [ ] `RequestModeRelease`
- [ ] `MiscellaneousIndication`
- [ ] `JitterIndication`
- [ ] `UserInputIndication`
- [ ] `H2250MaximumSkewIndication`
- [ ] `MCLocationIndication`
- [ ] `ConferenceIndication`
- [ ] `VendorIdentification`
- [ ] `FunctionNotSupported`
- [ ] `LogicalChannelRateRelease`
- [ ] `FlowControlIndication`
- [ ] Voicemail
- [ ] Real-Time Text
- [ ] Real-Time Emoji / React
- [ ] Drawing
- [ ] Geolocation
- [ ] Mute
- [ ] Jitter Buffer
- [ ] Call Hold
- [ ] Call Transfer
- [ ] Call Diversion
- [ ] Call Intrusion
- [ ] Call Waiting
- [ ] Call Completion
- [ ] Call Offer
- [ ] Video Calls
- [ ] Android App
- [ ] iOS App
- [ ] Apple TV App
- [ ] Browser App
- [ ] Desktop App
- [ ] Debian Package
- [ ] RPM Package
- [ ] Nix Flake
- [ ] Alpine APK
- [ ] Brew Package
- [ ] Chocolatey Package
- [ ] Docker Container
- [ ] SystemD Service
- [ ] Windows Service
- [ ] MacOS Service
- [ ] TOR Integration
- [ ] I2P Integration
- [ ] SASL Authentication
- [ ] C Library
- [ ] TypeScript Library
- [ ] Go Library
- [ ] Encryption per https://www.rfc-editor.org/info/rfc9335
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
It receives the RTP packets from this port and decodes them, but this is as
far as I got.

The "client" is basically just for testing. The code is intentionally the
crappiest and low-effort, because I just want to get the server working.

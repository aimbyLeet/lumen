//! The handshake with the host, before the network thread starts.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::Result;
use protocol::{
    Capabilities, ChromaFormat, Codec, ControlMessage, PacketHeader, PacketKind, VideoFormat,
    handshake_datagram,
};
use transport::{Initiator, Opener, Sealer, SeqCounter, UdpLink, wall_clock_us};

use super::Options;
use super::checks::sim_dropper;

/// No address of the host answered the handshake: none is reachable from
/// here (the tunnel is down, another VPN took the route), or the token is wrong.
#[derive(Debug)]
pub struct Unreachable {
    pub hosts: Vec<SocketAddr>,
    pub timeout: Duration,
}

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no answer from the host at {:?} within {:?} (not reachable from here: is the \
             WireGuard / Tailscale tunnel up? Another VPN on this machine or on the host? \
             Or a wrong token)",
            self.hosts, self.timeout
        )
    }
}

impl std::error::Error for Unreachable {}

/// What the client can show, said in `Hello`.
fn hello(fps: u16, keep_unlocked: bool) -> Vec<u8> {
    let caps = Capabilities {
        codecs: 0,
        chroma_formats: 0,
        max_bit_depth: 10,
        display_width: 0,
        display_height: 0,
    }
    .with_codec(Codec::HevcMain10)
    .with_chroma(ChromaFormat::Yuv420);
    let mut hello = Vec::new();
    ControlMessage::Hello {
        caps,
        fps,
        keep_unlocked,
    }
    .encode(&mut hello);
    hello
}

/// The handshake with the host (Noise IK): message 1 (with `Hello`) until
/// message 2 arrives, then the encrypted `StreamStart`. Message 1 is resent
/// unchanged, so a host that already answered it repeats the same answer and
/// keeps the keys.
pub(super) struct Handshake<'a> {
    opts: &'a Options,
    hello: Vec<u8>,
    initiator: Option<Initiator>,
    message1: Vec<u8>,
    keys: Option<(Sealer, Opener)>,
    session: u32,
    /// The address that answered; the link is connected to it.
    peer: Option<SocketAddr>,
    /// Our datagrams' numbers (0 is message 1's): the network thread goes on
    /// with it (a number is never used twice under one key).
    seq: SeqCounter,
    /// What the host says instead of a stream (no monitor).
    waiting: crate::monitor_pick::HostWaits,
}

/// The handshake done.
pub(super) struct Shaken {
    pub(super) peer: SocketAddr,
    pub(super) session: u32,
    pub(super) format: VideoFormat,
    pub(super) sealer: Sealer,
    pub(super) opener: Opener,
    pub(super) seq: SeqCounter,
}

/// What the handshake got from one datagram.
enum Took {
    Nothing,
    /// The host has no monitor (yet): it asks, or says there is none.
    Waiting,
    Started(VideoFormat),
}

impl<'a> Handshake<'a> {
    pub(super) fn new(opts: &'a Options) -> Result<Self> {
        let hello = hello(opts.fps, opts.keep_unlocked);
        let init = Initiator::new(&opts.key, &opts.host_key, &opts.psk, &hello)?;
        let seq = SeqCounter::new();
        // 0 is message 1's.
        seq.take();
        Ok(Handshake {
            message1: handshake_datagram(0, init.message()),
            initiator: Some(init),
            opts,
            hello,
            keys: None,
            session: 0,
            peer: None,
            seq,
            waiting: Default::default(),
        })
    }

    /// Message 1 to every address until one answers, then to that one.
    pub(super) fn run(
        mut self,
        link: &UdpLink,
        hosts: &[SocketAddr],
        timeout: Duration,
        sim_loss: f64,
    ) -> Result<Shaken> {
        let mut deadline = Instant::now() + timeout;
        let mut buf = vec![0u8; 2048];
        // --sim-loss applies to the handshake too (exercises the host's repeats).
        let mut sim_drop = sim_dropper(sim_loss, wall_clock_us() | 1);
        loop {
            if Instant::now() >= deadline {
                return Err(Unreachable {
                    hosts: hosts.to_vec(),
                    timeout,
                }
                .into());
            }
            match self.peer {
                Some(_) => link.send(&self.message1)?,
                None => {
                    for h in hosts {
                        // An address unreachable from here (another network) is no error.
                        let _ = link.send_to(&self.message1, *h);
                    }
                }
            }
            let until = Instant::now() + Duration::from_millis(200);
            while let Some((n, from)) =
                link.recv_from_timeout(&mut buf, until.saturating_duration_since(Instant::now()))?
            {
                if sim_drop() || self.peer.is_some_and(|p| p != from) {
                    continue;
                }
                match self.take(link, from, &mut buf[..n])? {
                    Took::Started(format) => {
                        let (sealer, opener) = self.keys.expect("StreamStart decrypted");
                        return Ok(Shaken {
                            peer: self.peer.expect("set with the keys"),
                            session: self.session,
                            format,
                            sealer,
                            opener,
                            seq: self.seq,
                        });
                    }
                    // The host is there and repeats itself: no deadline while
                    // it waits for a monitor (or for the user's answer).
                    Took::Waiting => deadline = Instant::now() + timeout,
                    Took::Nothing => {}
                }
                if Instant::now() >= until {
                    break;
                }
            }
        }
    }

    /// One datagram from `from`: message 2 (the keys), the `StreamStart`, or
    /// the host's word that it has no monitor yet.
    fn take(&mut self, link: &UdpLink, from: SocketAddr, datagram: &mut [u8]) -> Result<Took> {
        let Ok((h, body)) = PacketHeader::decode(datagram) else {
            return Ok(Took::Nothing);
        };
        match (&mut self.keys, h.kind) {
            (None, PacketKind::Handshake) => {
                let init = self.initiator.take().expect("present until keys are set");
                match init.finish(body) {
                    Ok(k) => {
                        self.keys = Some(k.split());
                        self.session = h.session;
                        link.connect(from)?;
                        self.peer = Some(from);
                    }
                    // Not a reply to us (forged, stale, or another token): start over.
                    Err(_) => {
                        let o = self.opts;
                        let init = Initiator::new(&o.key, &o.host_key, &o.psk, &self.hello)?;
                        self.message1 = handshake_datagram(0, init.message());
                        self.initiator = Some(init);
                    }
                }
            }
            (Some((_, opener)), PacketKind::Control) if h.session == self.session => {
                let Ok(len) = opener.open(datagram) else {
                    return Ok(Took::Nothing);
                };
                let Ok((_, body)) = PacketHeader::decode(&datagram[..len]) else {
                    return Ok(Took::Nothing);
                };
                match ControlMessage::decode(body) {
                    Ok(ControlMessage::StreamStart { stream: 0, format }) => {
                        return Ok(Took::Started(format));
                    }
                    Ok(m @ (ControlMessage::MonitorWanted { .. } | ControlMessage::NoMonitors)) => {
                        // The window does not open until a stream starts.
                        if let Some(answer) = self.waiting.heard(&m, false) {
                            self.send(link, &answer)?;
                        }
                        return Ok(Took::Waiting);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(Took::Nothing)
    }

    /// A control message to the host before the network thread runs.
    fn send(&self, link: &UdpLink, m: &ControlMessage) -> Result<()> {
        let (sealer, _) = self.keys.as_ref().expect("a control message decrypted");
        let mut d = m.datagram(self.session, self.seq.take());
        sealer
            .seal(&mut d)
            .expect("a datagram we built has a header");
        link.send(&d)?;
        Ok(())
    }
}

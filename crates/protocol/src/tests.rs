use super::*;
use crate::clipboard::{CLIP_PIECE_LEN, ClipPiece};

fn caps() -> Capabilities {
    Capabilities {
        codecs: 0,
        chroma_formats: 0,
        max_bit_depth: 10,
        display_width: 3840,
        display_height: 2160,
    }
    .with_codec(Codec::HevcMain10)
    .with_chroma(ChromaFormat::Yuv420)
}

/// One instance of every control message, with non-trivial field values.
fn all_messages() -> Vec<ControlMessage> {
    vec![
        ControlMessage::Hello {
            caps: caps(),
            fps: 30,
            keep_unlocked: false,
        },
        ControlMessage::Hello {
            caps: caps(),
            fps: 60,
            keep_unlocked: true,
        },
        ControlMessage::StreamStart {
            stream: 0,
            format: VideoFormat::lumen_default(3840, 2160, 60),
        },
        ControlMessage::StreamStart {
            stream: 3,
            format: VideoFormat {
                codec: Codec::Av1,
                chroma: ChromaFormat::Yuv444,
                fps_num: 60000,
                fps_den: 1001,
                ..VideoFormat::lumen_default(1920, 1080, 60)
            },
        },
        ControlMessage::RequestKeyframe {
            stream: 1,
            frame: 0xDEAD_BEEF,
        },
        ControlMessage::RequestRecovery {
            stream: 2,
            last_good: 41,
            lost: 0xFFFF_FFFE,
        },
        ControlMessage::Ack { seq: u64::MAX - 1 },
        ControlMessage::Monitors {
            generation: 7,
            monitors: vec![
                Monitor {
                    stream: 0,
                    x: 0,
                    y: 0,
                    width: 3840,
                    height: 2160,
                },
                Monitor {
                    stream: 1,
                    x: -3440,
                    y: 720,
                    width: 3440,
                    height: 1440,
                },
                Monitor {
                    stream: 3,
                    x: 3840,
                    y: -720,
                    width: 1920,
                    height: 1080,
                },
            ],
        },
        ControlMessage::Monitors {
            generation: 0,
            monitors: vec![],
        },
        ControlMessage::ShowMonitors {
            generation: 7,
            streams: 0b1010,
        },
        ControlMessage::MonitorWanted { ask: 3 },
        ControlMessage::MonitorChoice {
            ask: 3,
            width: 3440,
            height: 1440,
            scale: 125,
        },
        ControlMessage::NoMonitors,
        ControlMessage::Ping {
            id: 7,
            time_us: 1_234_567_890_123,
        },
        ControlMessage::Pong {
            id: 7,
            time_us: 1_234_567_890_123,
            host_us: 1_234_567_999_999,
        },
        ControlMessage::Stats(ClientStats {
            packets_received: 1,
            packets_lost: 2,
            fec_recovered: 3,
            frames_decoded: 4,
            frames_dropped: 5,
            keyframes_requested: 6,
            recoveries_requested: 7,
        }),
        ControlMessage::Input {
            id: 0,
            event: InputEvent::MouseMove { x: -5, y: 2159 },
        },
        ControlMessage::Input {
            id: 1,
            event: InputEvent::MouseButton {
                button: MouseButton::X2,
                down: true,
            },
        },
        ControlMessage::Input {
            id: u32::MAX,
            event: InputEvent::MouseButton {
                button: MouseButton::Left,
                down: false,
            },
        },
        ControlMessage::Input {
            id: 7,
            event: InputEvent::MouseWheel { dx: -120, dy: 240 },
        },
        ControlMessage::Input {
            id: 8,
            event: InputEvent::Key {
                scancode: 0x1D,
                extended: true,
                down: false,
            },
        },
        ControlMessage::Input {
            id: 9,
            event: InputEvent::SecureAttention,
        },
        ControlMessage::Input {
            id: 10,
            event: InputEvent::Clipboard(ClipPiece {
                last: false,
                data: vec![0xd1; CLIP_PIECE_LEN],
            }),
        },
        ControlMessage::Input {
            id: 11,
            event: InputEvent::Clipboard(ClipPiece {
                last: true,
                data: Vec::new(),
            }),
        },
        ControlMessage::Inputs {
            events: vec![
                (3, InputEvent::MouseMove { x: 1, y: 2 }),
                (
                    9,
                    InputEvent::Key {
                        scancode: 0x1E,
                        extended: false,
                        down: true,
                    },
                ),
                (10, InputEvent::SecureAttention),
            ],
        },
        ControlMessage::InputAck { last: 8 },
        ControlMessage::Clipboard {
            id: u32::MAX,
            piece: ClipPiece {
                last: true,
                data: "текст".as_bytes().to_vec(),
            },
        },
        ControlMessage::Clipboard {
            id: 3,
            piece: ClipPiece {
                last: false,
                data: vec![b'x'; CLIP_PIECE_LEN],
            },
        },
        ControlMessage::ClipboardAck { last: 3 },
        ControlMessage::HostStats(crate::control::HostStats {
            fps_x10: 599,
            video_kbps: 22_400,
            encoder_kbps: 46_000,
            cap_kbps: 40_000,
            qp_x10: 162,
            encode_p50_us: 2_600,
            encode_max_us: 3_550,
            queued: 18,
        }),
        ControlMessage::CursorPosition {
            x: 100,
            y: -1,
            visible: true,
            shape_id: 42,
        },
        ControlMessage::RequestCursorShape { id: 42 },
        ControlMessage::SetPacing {
            rate_kbps: 150_000,
            burst_bytes: 8192,
        },
        ControlMessage::SetFec {
            parity_permille: 50,
        },
        ControlMessage::Bye,
    ]
}

fn encode_msg(m: &ControlMessage) -> Vec<u8> {
    let mut v = Vec::new();
    m.encode(&mut v);
    v
}

fn header() -> PacketHeader {
    PacketHeader {
        kind: PacketKind::Video,
        flags: Flags::KEYFRAME | Flags::ACK_REQUESTED,
        stream: 2,
        session: 0x0102_0304,
        seq: 0x1122_3344_5566_7788,
    }
}

fn fragment() -> FragmentHeader {
    FragmentHeader {
        frame: 99,
        total_len: 123_456,
        capture_time_us: 987_654_321,
        block: 2,
        block_count: 3,
        index: 40,
        data_shards: 32,
        parity_shards: 9,
    }
}

fn cursors() -> Vec<CursorShape> {
    vec![
        CursorShape {
            id: 1,
            kind: CursorKind::Color,
            width: 3,
            height: 2,
            pitch: 12,
            hot_x: 1,
            hot_y: 1,
            data: (0..24).collect(),
        },
        CursorShape {
            id: 2,
            kind: CursorKind::Monochrome,
            width: 32,
            height: 64,
            pitch: 4,
            hot_x: 0,
            hot_y: 0,
            data: vec![0xAA; 256],
        },
        CursorShape {
            id: 3,
            kind: CursorKind::MaskedColor,
            width: 2,
            height: 1,
            pitch: 16, // padded rows are allowed
            hot_x: 0,
            hot_y: 0,
            data: vec![0xFF; 16],
        },
    ]
}

fn encode_cursor(c: &CursorShape) -> Vec<u8> {
    let mut v = Vec::new();
    c.encode(&mut v);
    v
}

// --- round trips -------------------------------------------------------------

#[test]
fn control_roundtrip_all() {
    for m in all_messages() {
        let bytes = encode_msg(&m);
        assert_eq!(ControlMessage::decode(&bytes).as_ref(), Ok(&m), "{m:?}");
    }
}

#[test]
fn control_messages_fit_one_datagram() {
    for m in all_messages() {
        let len = HEADER_LEN + encode_msg(&m).len() + AUTH_TAG_LEN;
        assert!(len <= DEFAULT_MAX_DATAGRAM, "{m:?}: {len}");
    }
}

#[test]
fn header_roundtrip_and_layout() {
    let mut v = Vec::new();
    header().encode(&mut v);
    assert_eq!(v.len(), HEADER_LEN);
    assert_eq!(&v[..3], &[b'L', b'M', PROTOCOL_VERSION]);
    v.extend_from_slice(b"body");
    let (h, body) = PacketHeader::decode(&v).unwrap();
    assert_eq!(h, header());
    assert_eq!(body, b"body");
}

#[test]
fn header_ignores_reserved_bytes() {
    let mut v = Vec::new();
    header().encode(&mut v);
    v[6..8].copy_from_slice(&[1, 2]);
    assert_eq!(PacketHeader::decode(&v).unwrap().0, header());
}

#[test]
fn fragment_roundtrip() {
    let mut v = Vec::new();
    fragment().encode(&mut v);
    assert_eq!(v.len(), FRAGMENT_HEADER_LEN);
    v.extend_from_slice(&[7; 10]);
    let (f, shard) = FragmentHeader::decode(&v).unwrap();
    assert_eq!(f, fragment());
    assert!(f.is_parity());
    assert_eq!(shard, &[7; 10]);
}

#[test]
fn full_video_datagram() {
    let shard_len = max_shard_len(DEFAULT_MAX_DATAGRAM);
    assert_eq!(shard_len % 2, 0);
    let shard: Vec<u8> = (0..shard_len).map(|i| i as u8).collect();

    let mut dgram = Vec::new();
    header().encode(&mut dgram);
    fragment().encode(&mut dgram);
    dgram.extend_from_slice(&shard);
    assert!(dgram.len() + AUTH_TAG_LEN <= DEFAULT_MAX_DATAGRAM);

    let (h, body) = PacketHeader::decode(&dgram).unwrap();
    assert_eq!(h.kind, PacketKind::Video);
    assert!(h.flags.contains(Flags::KEYFRAME));
    assert!(!h.flags.contains(Flags::ENCRYPTED));
    let (f, s) = FragmentHeader::decode(body).unwrap();
    assert_eq!(f, fragment());
    assert_eq!(s, &shard[..]);
}

#[test]
fn cursor_roundtrip() {
    for c in cursors() {
        assert_eq!(CursorShape::decode(&encode_cursor(&c)), Ok(c.clone()));
    }
}

#[test]
fn capabilities_accept() {
    let c = caps();
    assert!(c.accepts(&VideoFormat::lumen_default(3840, 2160, 60)));
    let av1 = VideoFormat {
        codec: Codec::Av1,
        ..VideoFormat::lumen_default(3840, 2160, 60)
    };
    assert!(!c.accepts(&av1));
    assert!(c.with_codec(Codec::Av1).accepts(&av1));
    let yuv444 = VideoFormat {
        chroma: ChromaFormat::Yuv444,
        ..VideoFormat::lumen_default(3840, 2160, 60)
    };
    assert!(!c.accepts(&yuv444));
}

#[test]
fn flags_ops() {
    let mut f = Flags::empty();
    f.set(Flags::ENCRYPTED, true);
    assert!(f.contains(Flags::ENCRYPTED));
    assert!(!f.contains(Flags::KEYFRAME));
    f.set(Flags::ENCRYPTED, false);
    assert_eq!(f, Flags::empty());
}

// --- rejection ---------------------------------------------------------------

#[test]
fn every_truncation_is_rejected() {
    for m in all_messages() {
        let bytes = encode_msg(&m);
        for n in 0..bytes.len() {
            assert_eq!(
                ControlMessage::decode(&bytes[..n]),
                Err(DecodeError::Truncated),
                "{m:?} cut at {n}"
            );
        }
    }
    let mut h = Vec::new();
    header().encode(&mut h);
    for n in 0..HEADER_LEN {
        assert_eq!(PacketHeader::decode(&h[..n]), Err(DecodeError::Truncated));
    }
    let mut f = Vec::new();
    fragment().encode(&mut f);
    f.push(0);
    for n in 0..f.len() {
        assert_eq!(FragmentHeader::decode(&f[..n]), Err(DecodeError::Truncated));
    }
    for c in cursors() {
        let bytes = encode_cursor(&c);
        for n in 0..bytes.len() {
            assert_eq!(
                CursorShape::decode(&bytes[..n]),
                Err(DecodeError::Truncated)
            );
        }
    }
}

#[test]
fn trailing_bytes_are_rejected() {
    for m in all_messages() {
        let mut bytes = encode_msg(&m);
        bytes.push(0);
        assert_eq!(
            ControlMessage::decode(&bytes),
            Err(DecodeError::TrailingBytes)
        );
    }
    for c in cursors() {
        let mut bytes = encode_cursor(&c);
        bytes.push(0);
        assert_eq!(CursorShape::decode(&bytes), Err(DecodeError::TrailingBytes));
    }
}

#[test]
fn header_rejects_foreign_packets() {
    let mut v = Vec::new();
    header().encode(&mut v);

    let mut stun = v.clone();
    stun[..2].copy_from_slice(&[0x00, 0x01]); // STUN binding request
    assert_eq!(PacketHeader::decode(&stun), Err(DecodeError::BadMagic));

    let mut ver = v.clone();
    ver[2] = PROTOCOL_VERSION + 1;
    assert_eq!(
        PacketHeader::decode(&ver),
        Err(DecodeError::UnsupportedVersion(PROTOCOL_VERSION + 1))
    );

    let mut kind = v.clone();
    kind[3] = 200;
    assert_eq!(
        PacketHeader::decode(&kind),
        Err(DecodeError::UnknownKind(200))
    );

    let mut stream = v.clone();
    stream[5] = MAX_STREAMS;
    assert_eq!(
        PacketHeader::decode(&stream),
        Err(DecodeError::InvalidValue("PacketHeader.stream"))
    );
}

#[test]
fn inputs_are_bounded() {
    let inputs = |n: usize| ControlMessage::Inputs {
        events: (0..n as u32)
            .map(|id| (id, InputEvent::MouseMove { x: 0, y: 0 }))
            .collect(),
    };
    let full = inputs(MAX_INPUTS);
    assert_eq!(ControlMessage::decode(&encode_msg(&full)), Ok(full));
    for n in [0, MAX_INPUTS + 1] {
        assert_eq!(
            ControlMessage::decode(&encode_msg(&inputs(n))),
            Err(DecodeError::InvalidValue("Inputs"))
        );
    }
}

#[test]
fn monitors_are_bounded() {
    let five = ControlMessage::Monitors {
        generation: 1,
        monitors: vec![
            Monitor {
                stream: 0,
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            };
            MAX_MONITORS + 1
        ],
    };
    assert_eq!(
        ControlMessage::decode(&encode_msg(&five)),
        Err(DecodeError::InvalidValue("Monitors"))
    );
    let bad_stream = encode_msg(&ControlMessage::RequestKeyframe {
        stream: MAX_STREAMS,
        frame: 1,
    });
    assert_eq!(
        ControlMessage::decode(&bad_stream),
        Err(DecodeError::InvalidValue("stream"))
    );
}

#[test]
fn control_rejects_bad_values() {
    assert_eq!(
        ControlMessage::decode(&[0]),
        Err(DecodeError::UnknownMessage(0))
    );
    assert_eq!(
        ControlMessage::decode(&[200]),
        Err(DecodeError::UnknownMessage(200))
    );
    // Input (tag 8, id 0) with MouseButton (event 2): unknown button, non-0/1 bool;
    // unknown event kind.
    for bad in [
        &[8, 0, 0, 0, 0, 2, 0, 1][..],
        &[8, 0, 0, 0, 0, 2, 1, 2],
        &[8, 0, 0, 0, 0, 9],
    ] {
        assert!(
            matches!(
                ControlMessage::decode(bad),
                Err(DecodeError::InvalidValue(_))
            ),
            "{bad:?}"
        );
    }

    let ok = VideoFormat::lumen_default(64, 64, 60);
    let bad = [
        VideoFormat {
            bit_depth: 12,
            ..ok
        },
        VideoFormat { width: 0, ..ok },
        VideoFormat { fps_den: 0, ..ok },
    ];
    for format in bad {
        let bytes = encode_msg(&ControlMessage::StreamStart { stream: 0, format });
        assert!(
            matches!(
                ControlMessage::decode(&bytes),
                Err(DecodeError::InvalidValue(_))
            ),
            "{format:?}"
        );
    }
}

#[test]
fn fragment_rejects_inconsistent_indices() {
    let ok = fragment();
    let bad = [
        FragmentHeader {
            block_count: 0,
            block: 0,
            ..ok
        },
        FragmentHeader { block: 3, ..ok },
        FragmentHeader {
            data_shards: 0,
            ..ok
        },
        FragmentHeader { index: 41, ..ok },
    ];
    for f in bad {
        let mut v = Vec::new();
        f.encode(&mut v);
        v.push(0);
        assert!(
            matches!(
                FragmentHeader::decode(&v),
                Err(DecodeError::InvalidValue(_))
            ),
            "{f:?}"
        );
    }
}

#[test]
fn cursor_rejects_bad_geometry() {
    let ok = cursors().remove(0);
    let bad = [
        CursorShape {
            width: 0,
            ..ok.clone()
        },
        CursorShape {
            height: MAX_CURSOR_DIM + 1,
            ..ok.clone()
        },
        CursorShape {
            pitch: 11,
            ..ok.clone()
        },
    ];
    for c in bad {
        assert!(
            matches!(
                CursorShape::decode(&encode_cursor(&c)),
                Err(DecodeError::InvalidValue(_))
            ),
            "{c:?}"
        );
    }
}

// --- fuzzing -----------------------------------------------------------------

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Runs every decoder on `bytes`; any panic fails the test. Successfully decoded
/// control messages and cursor shapes must re-encode to the same bytes.
fn decode_everything(bytes: &[u8]) {
    if let Ok((_, body)) = PacketHeader::decode(bytes) {
        let _ = FragmentHeader::decode(body);
        let _ = ControlMessage::decode(body);
    }
    let _ = FragmentHeader::decode(bytes);
    if let Ok(m) = ControlMessage::decode(bytes) {
        assert_eq!(encode_msg(&m), bytes, "non-canonical {m:?}");
    }
    if let Ok(c) = CursorShape::decode(bytes) {
        assert_eq!(encode_cursor(&c), bytes);
    }
}

#[test]
fn fuzz_random_bytes() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let len = rng.below(80);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        decode_everything(&bytes);
    }
}

#[test]
fn fuzz_mutated_valid_packets() {
    let mut seeds: Vec<Vec<u8>> = Vec::new();
    for m in all_messages() {
        seeds.push(encode_msg(&m));
        let mut d = Vec::new();
        PacketHeader {
            kind: PacketKind::Control,
            ..header()
        }
        .encode(&mut d);
        m.encode(&mut d);
        seeds.push(d);
    }
    for c in cursors() {
        seeds.push(encode_cursor(&c));
    }
    let mut d = Vec::new();
    header().encode(&mut d);
    fragment().encode(&mut d);
    d.extend_from_slice(&[1, 2, 3, 4]);
    seeds.push(d);

    let mut rng = XorShift(0xD1B5_4A32_D192_ED03);
    for _ in 0..50_000 {
        let mut bytes = seeds[rng.below(seeds.len())].clone();
        for _ in 0..1 + rng.below(3) {
            match rng.below(3) {
                0 if !bytes.is_empty() => {
                    let i = rng.below(bytes.len());
                    bytes[i] = rng.next() as u8;
                }
                1 if !bytes.is_empty() => {
                    let n = rng.below(bytes.len());
                    bytes.truncate(n);
                }
                _ => bytes.push(rng.next() as u8),
            }
        }
        decode_everything(&bytes);
    }
}

#[test]
fn clipboard_piece_longer_than_allowed_is_rejected() {
    let msg = ControlMessage::Clipboard {
        id: 1,
        piece: ClipPiece {
            last: true,
            data: vec![b'x'; CLIP_PIECE_LEN + 1],
        },
    };
    let mut body = Vec::new();
    msg.encode(&mut body);
    assert!(ControlMessage::decode(&body).is_err());
}

#[test]
fn clipboard_piece_truncated_fails() {
    let msg = ControlMessage::Clipboard {
        id: 1,
        piece: ClipPiece {
            last: true,
            data: vec![b'x'; 100],
        },
    };
    let mut body = Vec::new();
    msg.encode(&mut body);
    for cut in 1..body.len() {
        assert!(ControlMessage::decode(&body[..body.len() - cut]).is_err());
    }
}

#[test]
fn full_clipboard_piece_fits_a_datagram() {
    let piece = ClipPiece {
        last: false,
        data: vec![0; CLIP_PIECE_LEN],
    };
    // Sealing adds the AEAD tag after the header and body.
    let sealed = |m: &ControlMessage| m.datagram(1, 1).len() + AUTH_TAG_LEN;
    let input = ControlMessage::Input {
        id: 1,
        event: InputEvent::Clipboard(piece.clone()),
    };
    // The 9 bytes `CLIP_PIECE_LEN` leaves for the message: exactly full.
    assert_eq!(sealed(&input), DEFAULT_MAX_DATAGRAM);
    let from_host = ControlMessage::Clipboard { id: 1, piece };
    assert!(sealed(&from_host) <= DEFAULT_MAX_DATAGRAM);
}

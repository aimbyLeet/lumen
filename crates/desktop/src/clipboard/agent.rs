//! The clipboard of another process (`clip-agent`: the host runs as SYSTEM,
//! which does not see what programs put on the clipboard through OLE —
//! Explorer's files, some programs' text). Two
//! pipes, one each way: `up` from the agent (changes, answers), `down` to it
//! (what to write). A frame: `len: u32` (of the rest), `kind: u8`, `id: u32`,
//! then a [`Content`] (`encode_content`) or, for an answer, one byte (written).

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use super::{Ask, Clipboard, Content, IMAGE_LIMIT, ImageFormat, POLL};

mod frame {
    pub const CHANGE: u8 = 0;
    pub const SET: u8 = 1;
    pub const DONE: u8 = 2;
}

fn write_frame(w: &mut impl Write, kind: u8, id: u32, body: &[u8]) -> std::io::Result<()> {
    let mut f = Vec::with_capacity(9 + body.len());
    f.extend_from_slice(&((5 + body.len()) as u32).to_le_bytes());
    f.push(kind);
    f.extend_from_slice(&id.to_le_bytes());
    f.extend_from_slice(body);
    w.write_all(&f)?;
    w.flush()
}

/// The next frame: `(kind, id, body)`; `None` when the pipe closed.
fn read_frame(r: &mut impl Read) -> Option<(u8, u32, Vec<u8>)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).ok()?;
    let len = u32::from_le_bytes(len) as usize;
    // A picture of 64 MB at most, a little more for its header.
    if !(5..=(IMAGE_LIMIT + (1 << 20))).contains(&len) {
        return None;
    }
    let mut f = vec![0u8; len];
    r.read_exact(&mut f).ok()?;
    let id = u32::from_le_bytes(f[1..5].try_into().ok()?);
    Some((f[0], id, f[5..].to_vec()))
}

pub fn encode_content(c: &Content) -> Vec<u8> {
    let mut out = Vec::new();
    let put = |out: &mut Vec<u8>, b: &[u8]| {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    };
    match c {
        Content::Text(t) => {
            out.push(0);
            put(&mut out, t.as_bytes());
        }
        Content::Files(f) => {
            out.push(1);
            out.extend_from_slice(&(f.len() as u32).to_le_bytes());
            for p in f {
                put(&mut out, p.to_string_lossy().as_bytes());
            }
        }
        Content::Image(parts) => {
            out.push(2);
            out.extend_from_slice(&(parts.len() as u32).to_le_bytes());
            for (f, b) in parts {
                out.push(*f as u8);
                put(&mut out, b);
            }
        }
        Content::Pending(s) => {
            out.push(3);
            out.extend_from_slice(&s.to_le_bytes());
        }
    }
    out
}

pub fn decode_content(b: &[u8]) -> Option<Content> {
    let mut at = 1usize;
    let u32_at = |at: &mut usize| -> Option<u32> {
        let v = u32::from_le_bytes(b.get(*at..*at + 4)?.try_into().ok()?);
        *at += 4;
        Some(v)
    };
    let bytes_at = |at: &mut usize| -> Option<Vec<u8>> {
        let n = u32_at(at)? as usize;
        let v = b.get(*at..*at + n)?.to_vec();
        *at += n;
        Some(v)
    };
    let c = match *b.first()? {
        0 => Content::Text(String::from_utf8(bytes_at(&mut at)?).ok()?),
        1 => {
            let n = u32_at(&mut at)?;
            let mut f = Vec::new();
            for _ in 0..n {
                f.push(PathBuf::from(String::from_utf8(bytes_at(&mut at)?).ok()?));
            }
            Content::Files(f)
        }
        2 => {
            let n = u32_at(&mut at)?;
            let mut parts = Vec::new();
            for _ in 0..n {
                let f = ImageFormat::from_u8(*b.get(at)?)?;
                at += 1;
                parts.push((f, bytes_at(&mut at)?));
            }
            Content::Image(parts)
        }
        3 => Content::Pending(u32_at(&mut at)?),
        _ => return None,
    };
    (at == b.len()).then_some(c)
}

/// Answers the agent has not given yet, by the id of what was asked.
type Waiting = Arc<Mutex<HashMap<u32, Sender<bool>>>>;

impl Clipboard {
    /// The clipboard of a `clip-agent` at the other end of these pipes.
    pub fn over_pipes(up: File, down: File) -> Clipboard {
        let (changes_tx, changes) = mpsc::channel();
        let (asks, asks_rx) = mpsc::channel::<Ask>();
        let waiting = Waiting::default();
        {
            let waiting = waiting.clone();
            // Ends when the agent's end closes.
            std::thread::Builder::new()
                .name("clip-agent reader".into())
                .spawn(move || read_agent(up, &changes_tx, &waiting))
                .expect("spawn the clip-agent reader");
        }
        let thread = std::thread::Builder::new()
            .name("clip-agent writer".into())
            .spawn(move || write_agent(down, asks_rx, &waiting))
            .expect("spawn the clip-agent writer");
        Clipboard {
            changes,
            asks,
            window: 0,
            thread: Some(thread),
        }
    }
}

/// The agent's changes and answers, until its end closes.
fn read_agent(mut up: File, changes: &Sender<Content>, waiting: &Waiting) {
    while let Some((kind, id, body)) = read_frame(&mut up) {
        match kind {
            frame::CHANGE => {
                if let Some(c) = decode_content(&body)
                    && changes.send(c).is_err()
                {
                    return;
                }
            }
            frame::DONE => {
                if let Some(done) = waiting.lock().unwrap().remove(&id) {
                    let _ = done.send(body.first() == Some(&1));
                }
            }
            _ => {}
        }
    }
    eprintln!("clipboard: the clip-agent is gone");
}

/// What to write, down to the agent; dropping `down` tells it to go.
fn write_agent(mut down: File, asks: Receiver<Ask>, waiting: &Waiting) {
    let mut id = 0u32;
    for ask in asks {
        match ask {
            Ask::Set(c, done) => {
                id = id.wrapping_add(1);
                waiting.lock().unwrap().insert(id, done);
                if write_frame(&mut down, frame::SET, id, &encode_content(&c)).is_err() {
                    return;
                }
            }
            Ask::Stop => return,
        }
    }
}

/// A change as the host must see it: it does not see this user's network
/// drive letters (`crate::netpath`).
fn as_the_host_sees(c: Content) -> Content {
    match c {
        Content::Files(paths) => Content::Files(
            paths
                .iter()
                .map(|p| crate::netpath::network_path(p))
                .collect(),
        ),
        c => c,
    }
}

/// The agent's side (`lumen-host clip-agent`, run as the signed-in user):
/// this session's clipboard, its changes up, writes from down, until the
/// host closes the pipes.
pub fn serve_agent(up: File, mut down: File) -> Result<(), String> {
    let board = Clipboard::start(false)?;
    let up = Arc::new(Mutex::new(up));
    let closed = Arc::new(AtomicBool::new(false));
    let (sets_tx, sets) = mpsc::channel::<(u32, Content)>();
    {
        let closed = closed.clone();
        std::thread::spawn(move || {
            while let Some((kind, id, body)) = read_frame(&mut down) {
                if kind == frame::SET
                    && let Some(c) = decode_content(&body)
                    && sets_tx.send((id, c)).is_err()
                {
                    break;
                }
            }
            closed.store(true, Ordering::Relaxed);
        });
    }
    let send = |kind, id, body: &[u8]| write_frame(&mut *up.lock().unwrap(), kind, id, body);
    while !closed.load(Ordering::Relaxed) {
        for (id, c) in sets.try_iter() {
            let ok = board.set(c);
            if send(frame::DONE, id, &[u8::from(ok)]).is_err() {
                return Ok(());
            }
        }
        for c in board.changes() {
            if send(frame::CHANGE, 0, &encode_content(&as_the_host_sees(c))).is_err() {
                return Ok(());
            }
        }
        std::thread::sleep(POLL / 2);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_cross_the_pipe() {
        for c in [
            Content::Text("цвет 🎨".into()),
            Content::Files(vec![r"C:\кадры\001.exr".into(), r"D:\x".into()]),
            Content::Image(vec![
                (ImageFormat::Png, vec![1, 2, 3]),
                (ImageFormat::DibV5, vec![]),
            ]),
            Content::Pending(9),
        ] {
            assert_eq!(decode_content(&encode_content(&c)), Some(c));
        }
        assert_eq!(decode_content(&[7]), None);
        let mut b = encode_content(&Content::Text("x".into()));
        b.push(0);
        assert_eq!(decode_content(&b), None, "trailing bytes");
        let mut pipe = Vec::new();
        write_frame(&mut pipe, frame::SET, 42, &[1, 2]).unwrap();
        assert_eq!(
            read_frame(&mut pipe.as_slice()),
            Some((frame::SET, 42, vec![1, 2]))
        );
    }
}

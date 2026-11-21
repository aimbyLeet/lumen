//! Files this side sends: offered, read and hashed as they go out, done
//! when the other side says it saved them.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Instant;

use protocol::{FileCancel, FileMsg, FilePacket, Purpose};
use ring::digest::{Context, SHA256};
use transport::TransferState;

use super::{Session, Stage, View, Worker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutState {
    Offered,
    Sending,
    /// Every byte acknowledged, `Done` sent.
    Sent,
    Saved,
    Over(FileCancel),
}

impl OutState {
    /// Not over yet: offered, going, or waiting for the other side's check.
    pub(super) fn going(self) -> bool {
        matches!(self, OutState::Offered | OutState::Sending | OutState::Sent)
    }
}

pub(super) struct Out {
    pub(super) purpose: Purpose,
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) len: u64,
    pub(super) file: Option<File>,
    pub(super) hash: Context,
    /// Bytes fed to `hash`, from the start.
    pub(super) hashed: u64,
    pub(super) state: OutState,
    pub(super) started: Instant,
    pub(super) ended: Option<Instant>,
}

impl Out {
    fn new(purpose: Purpose, name: String, path: PathBuf, len: u64, file: Option<File>) -> Out {
        Out {
            purpose,
            name,
            path,
            len,
            file,
            hash: Context::new(&SHA256),
            hashed: 0,
            state: OutState::Offered,
            started: Instant::now(),
            ended: None,
        }
    }

    /// A row of a paste that could not start (nothing goes).
    pub(super) fn refused(name: String, reason: FileCancel, now: Instant) -> Out {
        Out {
            state: OutState::Over(reason),
            started: now,
            ended: Some(now),
            ..Out::new(
                Purpose::Download { batch: 0 },
                name,
                PathBuf::new(),
                0,
                None,
            )
        }
    }

    /// The hash made to cover the first `from` bytes again (going on from
    /// there after a reconnection), read from `file`.
    fn hash_up_to(&mut self, file: &mut File, from: u64) -> io::Result<()> {
        self.hash = Context::new(&SHA256);
        self.hashed = 0;
        let mut buf = vec![0u8; 1 << 20];
        file.seek(SeekFrom::Start(0))?;
        while self.hashed < from {
            let n = buf.len().min((from - self.hashed) as usize);
            file.read_exact(&mut buf[..n])?;
            self.hash.update(&buf[..n]);
            self.hashed += n as u64;
        }
        Ok(())
    }

    /// `buf` read at `offset` for the wire; the hash takes it if it is next.
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let f = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("not open"))?;
        f.seek(SeekFrom::Start(offset))?;
        f.read_exact(buf)?;
        if offset == self.hashed {
            self.hash.update(buf);
            self.hashed += buf.len() as u64;
        }
        Ok(())
    }

    /// Every byte out: what the other side checks it against.
    fn sha256(&self, id: u32) -> [u8; 32] {
        if self.hashed != self.len {
            eprintln!(
                "files: #{id}: hashed {} of {} bytes (the file changed?)",
                self.hashed, self.len
            );
        }
        self.hash
            .clone()
            .finish()
            .as_ref()
            .try_into()
            .expect("SHA-256")
    }

    /// Its row in the overlay; `acked`: the bytes the other side has.
    pub(super) fn view(&self, id: u32, acked: u64) -> View {
        let (stage, done) = match self.state {
            OutState::Offered => (Stage::Waiting, acked),
            OutState::Sending => (Stage::Running, acked),
            OutState::Sent => (Stage::Checking, self.len),
            OutState::Saved => (Stage::Saved, self.len),
            OutState::Over(r) => (Stage::Cancelled(r), acked),
        };
        View {
            outgoing: true,
            download: matches!(self.purpose, Purpose::Download { .. }),
            set: self.purpose.clip_set(),
            batch: match self.purpose {
                Purpose::Download { batch } => Some(batch),
                _ => None,
            },
            image: matches!(self.purpose, Purpose::Image { .. }),
            transfer: id,
            name: self.name.clone(),
            len: self.len,
            done,
            stage,
            speed: 0.0,
            started: self.started,
            ended: self.ended,
        }
    }
}

impl Worker {
    /// Offers a file (`file`: already open; it is read from there); returns
    /// its transfer.
    pub(super) fn offer_new(
        &mut self,
        path: PathBuf,
        name: String,
        purpose: Purpose,
        file: Option<File>,
    ) -> Option<u32> {
        let meta = match &file {
            Some(f) => f.metadata(),
            None => std::fs::metadata(&path),
        };
        let len = match meta {
            Ok(m) => m.len(),
            Err(e) => {
                eprintln!("files: {}: {e}", path.display());
                return None;
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        println!("files: offer #{id} {name} ({len} bytes, {purpose:?})");
        self.out
            .insert(id, Out::new(purpose, name.clone(), path, len, file));
        self.msg(FileMsg::Offer {
            transfer: id,
            len,
            name,
            purpose,
        });
        Some(id)
    }

    /// What is not saved yet is offered again (a session reconnected); the
    /// receiver says from where.
    pub(super) fn offer_again(&mut self) {
        let mut again: Vec<(u32, u64, String, Purpose)> = Vec::new();
        for (&id, o) in self.out.iter_mut() {
            if o.state.going() {
                o.state = OutState::Offered;
                again.push((id, o.len, o.name.clone(), o.purpose));
            }
        }
        again.sort_by_key(|a| a.0);
        for (transfer, len, name, purpose) in again {
            self.msg(FileMsg::Offer {
                transfer,
                len,
                name,
                purpose,
            });
        }
    }

    pub(super) fn on_accept(&mut self, transfer: u32, from: u64) {
        let Some(o) = self.out.get_mut(&transfer) else {
            return;
        };
        if o.state != OutState::Offered {
            return;
        }
        let opened = match o.file.take() {
            Some(f) => Ok(f),
            None => File::open(&o.path),
        };
        let from = from.min(o.len);
        let ready = opened.and_then(|mut file| {
            // Resuming: the hash covers the start again, read here.
            if o.hashed != from {
                o.hash_up_to(&mut file, from)?;
            }
            Ok(file)
        });
        let file = match ready {
            Ok(f) => f,
            Err(e) => {
                eprintln!("files: #{transfer} {}: {e}", o.path.display());
                self.cancel(true, transfer, FileCancel::Read, true);
                return;
            }
        };
        if from > 0 {
            println!("files: #{transfer} {} goes on from byte {from}", o.name);
        }
        o.file = Some(file);
        o.state = OutState::Sending;
        if let Some(s) = self.session.as_mut() {
            s.tx.start_from(transfer, o.len, from);
        }
    }

    /// Our data out, acks out, sent transfers finished.
    pub(super) fn pump(&mut self, now: Instant) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        if s.rx.ack_due(now) {
            (s.io.data)(&FilePacket::Ack(s.rx.ack(now)));
        }
        let out = &mut self.out;
        let mut read = |t: u32, offset: u64, buf: &mut [u8]| -> io::Result<()> {
            out.get_mut(&t)
                .ok_or_else(|| io::Error::other("no such transfer"))?
                .read(offset, buf)
        };
        while let Some(d) = s.tx.poll(now, &mut read) {
            (s.io.data)(&FilePacket::Data(d));
        }
        let sending = finish_sent(s, &mut self.out, now);
        self.sending.store(sending, Ordering::Relaxed);
    }

    pub(super) fn cancel_out(&mut self, id: u32, reason: FileCancel, now: Instant) -> bool {
        let Some(o) = self.out.get_mut(&id) else {
            return false;
        };
        if !o.state.going() {
            return false;
        }
        o.state = OutState::Over(reason);
        o.file = None;
        o.ended = Some(now);
        println!("files: #{id} {} not sent: {reason:?}", o.name);
        if let Some(s) = self.session.as_mut() {
            s.tx.cancel(id);
        }
        true
    }

    /// The other side saved our transfer.
    pub(super) fn saved_there(&mut self, transfer: u32) {
        if let Some(o) = self.out.get_mut(&transfer)
            && o.state != OutState::Saved
        {
            o.state = OutState::Saved;
            o.file = None;
            o.ended = Some(Instant::now());
            println!(
                "files: sent #{transfer} {} ({} bytes) in {:.1} s, {:.1} Mbit/s",
                o.name,
                o.len,
                o.started.elapsed().as_secs_f64(),
                super::names::mbps(o.len, o.started)
            );
        }
    }
}

/// Transfers all out get `Done` (with their hash), failed ones `Cancel`;
/// returns whether any still sends.
fn finish_sent(s: &Session, out: &mut std::collections::HashMap<u32, Out>, now: Instant) -> bool {
    let mut sending = false;
    for (&id, o) in out.iter_mut() {
        if o.state != OutState::Sending {
            continue;
        }
        match s.tx.progress(id).map(|p| p.state) {
            Some(TransferState::Done) => {
                o.state = OutState::Sent;
                (s.io.msg)(FileMsg::Done {
                    transfer: id,
                    sha256: o.sha256(id),
                });
            }
            Some(TransferState::Failed) => {
                o.state = OutState::Over(FileCancel::Read);
                o.ended = Some(now);
                (s.io.msg)(FileMsg::Cancel {
                    transfer: id,
                    mine: true,
                    reason: FileCancel::Read,
                });
            }
            _ => sending = true,
        }
    }
    sending
}

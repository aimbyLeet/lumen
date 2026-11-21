//! Files the other side sends: `<name>.part`, written as chunks come,
//! hashed in order, checked against the sender's SHA-256 and renamed.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use protocol::{FILE_CHUNK_LEN, FileCancel, FileData, FileMsg, FilePacket, Purpose};
use ring::digest::{Context, SHA256};

use super::names::{free_bytes, free_path, hex, mbps, part_of, safe_name};
use super::{HASH_STEP, Stage, View, Worker};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InState {
    Receiving,
    Saved(PathBuf),
    Over(FileCancel),
}

pub(super) struct In {
    pub(super) purpose: Purpose,
    pub(super) name: String,
    pub(super) len: u64,
    pub(super) part: PathBuf,
    target: PathBuf,
    pub(super) file: Option<File>,
    /// Chunks from the start that are all here; ones past them that are.
    pub(super) contiguous: u64,
    pub(super) have: BTreeSet<u64>,
    hash: Context,
    hashed: u64,
    pub(super) expect: Option<[u8; 32]>,
    pub(super) state: InState,
    pub(super) started: Instant,
    pub(super) ended: Option<Instant>,
}

impl In {
    pub(super) fn new(
        purpose: Purpose,
        name: String,
        len: u64,
        target: PathBuf,
        file: Option<File>,
    ) -> In {
        In {
            purpose,
            name,
            len,
            part: part_of(&target),
            target,
            file,
            contiguous: 0,
            have: BTreeSet::new(),
            hash: Context::new(&SHA256),
            hashed: 0,
            expect: None,
            state: InState::Receiving,
            started: Instant::now(),
            ended: None,
        }
    }

    /// A row of a paste that could not start (nothing comes).
    pub(super) fn refused(name: String, reason: FileCancel, now: Instant) -> In {
        In {
            part: PathBuf::new(),
            state: InState::Over(reason),
            started: now,
            ended: Some(now),
            ..In::new(
                Purpose::Download { batch: 0 },
                name,
                0,
                PathBuf::new(),
                None,
            )
        }
    }

    pub(super) fn bytes_here(&self) -> u64 {
        (self.contiguous * FILE_CHUNK_LEN as u64).min(self.len)
    }

    /// Chunk `i` arrived.
    pub(super) fn got(&mut self, i: u64) {
        if i < self.contiguous {
            return;
        }
        self.have.insert(i);
        while self.have.remove(&self.contiguous) {
            self.contiguous += 1;
        }
    }

    fn write_at(&mut self, d: &FileData) -> io::Result<()> {
        let f = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("closed"))?;
        f.seek(SeekFrom::Start(d.offset))?;
        f.write_all(&d.data)
    }

    /// The next bytes that are all here, into the hash (at most `HASH_STEP`).
    fn hash_step(&mut self) -> io::Result<()> {
        let here = self.bytes_here();
        if self.hashed >= here {
            return Ok(());
        }
        let n = ((here - self.hashed) as usize).min(HASH_STEP);
        let mut buf = vec![0u8; n];
        let f = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("closed"))?;
        f.seek(SeekFrom::Start(self.hashed))?;
        f.read_exact(&mut buf)?;
        self.hash.update(&buf);
        self.hashed += n as u64;
        Ok(())
    }

    /// All hashed and the sender's hash known: whether they match.
    fn checked(&self) -> Option<bool> {
        let expect = self.expect?;
        (self.hashed == self.len).then(|| self.hash.clone().finish().as_ref() == expect)
    }

    /// Its row in the overlay.
    pub(super) fn view(&self, id: u32) -> View {
        let (stage, done) = match &self.state {
            InState::Receiving if self.bytes_here() == self.len => (Stage::Checking, self.len),
            InState::Receiving => (
                Stage::Running,
                (self.contiguous + self.have.len() as u64) * FILE_CHUNK_LEN as u64,
            ),
            InState::Saved(_) => (Stage::Saved, self.len),
            InState::Over(r) => (Stage::Cancelled(*r), self.bytes_here()),
        };
        View {
            outgoing: false,
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
            done: done.min(self.len),
            stage,
            speed: 0.0,
            started: self.started,
            ended: self.ended,
        }
    }
}

/// `<target>.part`, `len` long, in a folder made if missing — unless the
/// disk has no room for it.
fn open_part(target: &Path, part: &Path, len: u64, folder: &Path) -> io::Result<File> {
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if free_bytes(target.parent().unwrap_or(folder)).is_some_and(|f| f < len) {
        return Err(io::Error::new(io::ErrorKind::StorageFull, "no room"));
    }
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(part)?;
    f.set_len(len)?;
    Ok(f)
}

impl Worker {
    fn refuse(&self, transfer: u32, reason: FileCancel) {
        self.msg(FileMsg::Cancel {
            transfer,
            mine: false,
            reason,
        });
    }

    /// Offered again by a session that reconnected: answered as it stands.
    /// False: not one we know (or one to receive anew).
    fn offered_again(&self, transfer: u32, len: u64) -> bool {
        let Some(i) = self.incoming.get(&transfer) else {
            return false;
        };
        match &i.state {
            InState::Receiving if i.len == len => {
                let from = i.bytes_here();
                self.msg(FileMsg::Accept { transfer, from });
            }
            InState::Saved(_) => self.msg(FileMsg::Saved { transfer }),
            InState::Over(r) => self.refuse(transfer, *r),
            InState::Receiving => return false,
        }
        true
    }

    /// Where an offered file goes: the downloads, or its clipboard set's folder.
    fn target_of(&self, purpose: Purpose, name: &str, folder: &Path) -> Option<PathBuf> {
        Some(match purpose {
            Purpose::Download { .. } => free_path(&folder.join(safe_name(name))),
            Purpose::Clip { set } => self.clip_dir(set)?.join(safe_name(name)),
            Purpose::Image { set, format } => self.clip_dir(set)?.join(format!("image-{format}")),
        })
    }

    pub(super) fn on_offer(&mut self, transfer: u32, len: u64, name: String, purpose: Purpose) {
        if self.offered_again(transfer, len) {
            return;
        }
        if !self.allowed && !matches!(purpose, Purpose::Image { .. }) {
            println!("files: #{transfer} {name} refused: file transfers are off (the admin)");
            self.refuse(transfer, FileCancel::NotAllowed);
            return;
        }
        if let Some(reason) = self.unavailable() {
            println!("files: #{transfer} {name} refused: {reason:?}");
            self.refuse(transfer, reason);
            return;
        }
        let Some(folder) = self.folder.clone() else {
            return;
        };
        let Some(target) = self.target_of(purpose, &name, &folder) else {
            self.refuse(transfer, FileCancel::NoUser);
            return;
        };
        let part = part_of(&target);
        let file = match open_part(&target, &part, len, &folder) {
            Ok(f) => f,
            Err(e) => {
                let reason = if e.kind() == io::ErrorKind::StorageFull {
                    FileCancel::Disk
                } else {
                    FileCancel::Write
                };
                println!("files: #{transfer} {name} refused: {e}");
                self.refuse(transfer, reason);
                return;
            }
        };
        println!(
            "files: receiving #{transfer} {name} ({len} bytes) into {}",
            part.display()
        );
        self.incoming
            .insert(transfer, In::new(purpose, name, len, target, Some(file)));
        self.msg(FileMsg::Accept { transfer, from: 0 });
    }

    pub(super) fn packet(&mut self, at: Instant, p: FilePacket) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        match p {
            FilePacket::Ack(a) => s.tx.on_ack(at, &a),
            FilePacket::Data(d) => {
                // Data of a transfer not (or no longer) received is not
                // acknowledged: the sender stops once it hears why.
                let Some(i) = self.incoming.get_mut(&d.transfer) else {
                    return;
                };
                if i.state != InState::Receiving || d.offset + d.data.len() as u64 > i.len {
                    return;
                }
                if !s.rx.on_data(at, &d) {
                    return;
                }
                if let Err(e) = i.write_at(&d) {
                    eprintln!("files: #{} {}: {e}", d.transfer, i.part.display());
                    self.cancel(false, d.transfer, FileCancel::Write, true);
                    return;
                }
                i.got(d.offset / FILE_CHUNK_LEN as u64);
            }
        }
    }

    /// Received bytes into the hash, in order; a complete file checked and
    /// put in its place.
    pub(super) fn hash_incoming(&mut self) {
        let mut finished = Vec::new();
        for (&id, i) in self.incoming.iter_mut() {
            if i.state != InState::Receiving {
                continue;
            }
            if let Err(e) = i.hash_step() {
                eprintln!("files: #{id} {}: {e}", i.part.display());
                finished.push((id, Some(FileCancel::Write)));
                continue;
            }
            if let Some(same) = i.checked() {
                finished.push((id, (!same).then_some(FileCancel::Hash)));
            }
        }
        for (id, failed) in finished {
            match failed {
                Some(reason) => {
                    if reason == FileCancel::Hash {
                        eprintln!("files: #{id}: SHA-256 differs from the sender's");
                    }
                    self.cancel(false, id, reason, true);
                }
                None => self.save(id),
            }
        }
    }

    fn save(&mut self, id: u32) {
        let i = self.incoming.get_mut(&id).expect("listed");
        if let Some(mut f) = i.file.take() {
            let _ = f.flush();
        }
        // The name may have been taken meanwhile.
        let target = if i.target.exists() {
            free_path(&i.target)
        } else {
            i.target.clone()
        };
        if let Err(e) = std::fs::rename(&i.part, &target) {
            eprintln!(
                "files: #{id} {} -> {}: {e}",
                i.part.display(),
                target.display()
            );
            let _ = std::fs::remove_file(&i.part);
            i.state = InState::Over(FileCancel::Write);
            i.ended = Some(Instant::now());
            self.refuse(id, FileCancel::Write);
            return;
        }
        println!(
            "files: saved #{id} {} ({} bytes) in {:.1} s, {:.1} Mbit/s, SHA-256 {}: {}",
            i.name,
            i.len,
            i.started.elapsed().as_secs_f64(),
            mbps(i.len, i.started),
            hex(i.hash.clone().finish().as_ref()),
            target.display()
        );
        i.state = InState::Saved(target.clone());
        i.ended = Some(Instant::now());
        let (purpose, name) = (i.purpose, i.name.clone());
        self.msg(FileMsg::Saved { transfer: id });
        if let Purpose::Clip { set } | Purpose::Image { set, .. } = purpose {
            self.set_part(set, purpose, name, target);
        }
    }

    pub(super) fn cancel_in(&mut self, id: u32, reason: FileCancel, now: Instant) -> bool {
        let Some(i) = self.incoming.get_mut(&id) else {
            return false;
        };
        if i.state != InState::Receiving {
            return false;
        }
        i.file = None;
        let _ = std::fs::remove_file(&i.part);
        i.state = InState::Over(reason);
        i.ended = Some(now);
        println!("files: #{id} {} not received: {reason:?}", i.name);
        true
    }

    /// The thread ends: unfinished files are not left behind.
    pub(super) fn drop_unfinished(&mut self) {
        for i in self.incoming.values_mut() {
            if i.state == InState::Receiving {
                i.file = None;
                let _ = std::fs::remove_file(&i.part);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_chunks() {
        let mut i = In::new(
            Purpose::Download { batch: 1 },
            String::new(),
            10 * FILE_CHUNK_LEN as u64,
            PathBuf::new(),
            None,
        );
        for c in [2, 1, 5] {
            i.got(c);
        }
        assert_eq!(i.contiguous, 0);
        i.got(0);
        assert_eq!(i.contiguous, 3);
        assert_eq!(i.bytes_here(), 3 * FILE_CHUNK_LEN as u64);
        i.got(1);
        assert_eq!(i.have.len(), 1);
    }
}

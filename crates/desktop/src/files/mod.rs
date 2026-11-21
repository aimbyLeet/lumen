//! File transfers of one side of a session, both ways:
//! the files this side sends and the ones it receives into
//! `Downloads\Lumen`.
//!
//! A thread of its own ([`FileService`]) outlives sessions: a session that
//! reconnects attaches again and the transfers go on from where the receiver
//! is (`FileMsg::Offer` → `FileMsg::Accept { from }`). The data goes through
//! `transport::BulkSender` / `BulkReceiver`; a file arrives as `<name>.part`,
//! is checked against the sender's SHA-256 and renamed. On the host, which
//! runs as SYSTEM, the thread works as the signed-in user (impersonation):
//! it reads only what the user may read, and what it writes is theirs.
//!
//! The thread's state is [`Worker`]; its parts: `outgoing` (what this side
//! sends), `incoming` (what it receives), `clip` (clipboard sets), `place`
//! (where files go and as whom), `views` (the overlay's rows), `names`.

mod clip;
mod incoming;
mod names;
mod outgoing;
mod place;
mod views;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use protocol::{FileCancel, FileMsg, FilePacket, Purpose};
use transport::{BulkConfig, BulkReceiver, BulkSender};

use clip::{GoingCopy, InSet};
use incoming::{In, InState};
use outgoing::Out;

pub use names::safe_name;
pub use place::{Place, PlaceFn, UserToken, downloads};

/// Files copied that come at once, onto the other side's clipboard: up to
/// this much in all (the owner's choice); more comes on a paste there, into
/// `Downloads\Lumen`.
pub const CLIP_LIMIT: u64 = 32 << 20;
/// The longest the thread sleeps while something is going on (the pacer).
const NAP: Duration = Duration::from_millis(1);
/// Bytes of a received file hashed per round (the rest waits for the next).
const HASH_STEP: usize = 4 << 20;
/// How often the views are refreshed.
const VIEW_EVERY: Duration = Duration::from_millis(200);
/// How often the log gets a line per running transfer.
const LOG_EVERY: Duration = Duration::from_secs(5);
/// How often the place (who is signed in, whether locked) is asked again.
const PLACE_EVERY: Duration = Duration::from_secs(1);
/// Refused pastes closer than this get one row.
const REFUSAL_EVERY: Duration = Duration::from_secs(1);

/// What the service tells the session's clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Something was copied on the other side, set `set`: the mark goes on
    /// the clipboard until it is here (`big`: until a paste fetches it into
    /// the downloads).
    Pending { set: u32, big: bool },
    /// Set `.0` came: these files and folders go on the clipboard.
    Files(u32, Vec<PathBuf>),
    /// Set `.0` came: this picture (its formats) goes on the clipboard.
    Image(u32, Vec<(u8, Vec<u8>)>),
    /// The other side put our set `.0` on its clipboard.
    Placed(u32),
}

/// What a session gives the service to talk to the other side.
pub struct Io {
    /// Seals and sends a `File` datagram.
    pub data: Box<dyn Fn(&FilePacket) + Send>,
    /// Queues a message on the reliable channel.
    pub msg: Box<dyn Fn(FileMsg) + Send>,
    /// To the session's clipboard.
    pub event: Box<dyn Fn(Event) + Send>,
}

pub enum Cmd {
    /// A session (re)started with `peer` (its key: another peer than before
    /// cancels what was going on). `cap_bps`: the most our data may take.
    Attach {
        /// The session's id (datagram headers): what a `Detach` names.
        session: u32,
        io: Io,
        peer: Vec<u8>,
        cap_bps: u64,
        place: PlaceFn,
        /// Files may go either way (the admin's switch; pictures always may).
        allowed: bool,
    },
    /// The admin changed the switch or the cap: off stops what goes at once.
    Policy {
        allowed: bool,
        cap_bps: u64,
    },
    /// Session `.0` is gone for now (it may reconnect). A newer one may
    /// already be attached (a client opens the new session before the old
    /// one's thread ends): then nothing changes.
    Detach(u32),
    Packet(Instant, FilePacket),
    Msg(FileMsg),
    /// Send these files and folders (into the other side's downloads).
    Send(Vec<PathBuf>),
    /// Files and folders copied here, clipboard set `set`.
    Copy {
        set: u32,
        paths: Vec<PathBuf>,
    },
    /// A picture copied here (its formats: `ImageFormat` numbers and bytes).
    CopyImage {
        set: u32,
        formats: Vec<(u8, Vec<u8>)>,
    },
    /// A paste here while the mark of the other side's set `.0` was on the
    /// clipboard: a big set is fetched.
    Paste(u32),
    /// The other side's set `.0` is on our clipboard now.
    Placed(u32),
    Cancel {
        outgoing: bool,
        transfer: u32,
    },
    /// The session ended for good (the video window closed).
    CancelAll,
    /// The program ends: unfinished files are dropped.
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Waiting for the other side to take it.
    Waiting,
    Running,
    /// All bytes there, the check pending.
    Checking,
    Saved,
    Cancelled(FileCancel),
}

/// A transfer as the overlay shows it.
#[derive(Debug, Clone)]
pub struct View {
    pub outgoing: bool,
    /// Into `Downloads\Lumen` (not a clipboard set's).
    pub download: bool,
    /// The clipboard set it belongs to (numbered per direction).
    pub set: Option<u32>,
    /// A download's batch (the files of one paste, numbered by the sender).
    pub batch: Option<u32>,
    /// A format of a copied picture, not a file.
    pub image: bool,
    pub transfer: u32,
    pub name: String,
    pub len: u64,
    pub done: u64,
    pub stage: Stage,
    /// Bytes per second lately.
    pub speed: f64,
    pub started: Instant,
    pub ended: Option<Instant>,
}

/// The service: its thread, its commands, what the overlay reads.
pub struct FileService {
    tx: Sender<Cmd>,
    views: Arc<Mutex<Vec<View>>>,
    sending: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FileService {
    pub fn start() -> FileService {
        let (tx, rx) = mpsc::channel();
        let views = Arc::new(Mutex::new(Vec::new()));
        let sending = Arc::new(AtomicBool::new(false));
        let w = Worker::new(rx, views.clone(), sending.clone());
        let thread = std::thread::Builder::new()
            .name("files".into())
            .spawn(move || w.run())
            .expect("spawn the files thread");
        FileService {
            tx,
            views,
            sending,
            thread: Some(thread),
        }
    }

    pub fn commands(&self) -> Sender<Cmd> {
        self.tx.clone()
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn views(&self) -> Vec<View> {
        self.views.lock().unwrap().clone()
    }

    /// The views as the thread keeps them (for another thread to read).
    pub fn views_handle(&self) -> Arc<Mutex<Vec<View>>> {
        self.views.clone()
    }

    /// Our data is going out now (the host keeps its video low meanwhile).
    pub fn sending(&self) -> Arc<AtomicBool> {
        self.sending.clone()
    }
}

impl Drop for FileService {
    fn drop(&mut self) {
        // Other senders (a session's) may still be around: said, not implied.
        let _ = self.tx.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Session {
    id: u32,
    io: Io,
    tx: BulkSender,
    rx: BulkReceiver,
}

struct Worker {
    rx: Receiver<Cmd>,
    views: Arc<Mutex<Vec<View>>>,
    sending: Arc<AtomicBool>,
    session: Option<Session>,
    peer: Option<Vec<u8>>,
    /// Where received files go; `None`: nobody to receive for.
    folder: Option<PathBuf>,
    /// Where clipboard sets go (theirs) and come from (ours).
    temp: Option<PathBuf>,
    /// Tells this run's clipboard folders apart from older ones.
    run_tag: u32,
    /// The other side's clipboard sets, the newest of them.
    in_sets: HashMap<u32, InSet>,
    latest_in: Option<u32>,
    /// Our big sets, waiting for a paste there (only the newest is kept).
    big_out: HashMap<u32, Vec<(PathBuf, String)>>,
    /// The transfers a paste there started, per big set of ours: another
    /// paste sends the set again once they are over.
    fetches: HashMap<u32, Vec<u32>>,
    /// Our newest set, copied while transfers were off: it goes once they
    /// are on.
    blocked: Option<(u32, Vec<PathBuf>)>,
    /// Our newest set has a file this side could not read: a paste there
    /// is refused (`FileCancel::Read`), not left waiting for it.
    unreadable: Option<u32>,
    /// Our newest small set going: what its files were (path, length,
    /// modified), and the sets copied again with the same files meanwhile —
    /// they go on as it (placed when it is).
    going_copy: Option<GoingCopy>,
    /// Files may go (the admin's switch).
    allowed: bool,
    place_fn: Option<PlaceFn>,
    place_checked: Option<Instant>,
    last_refusal: Option<Instant>,
    /// Windows is locked (the host).
    locked: bool,
    impersonating: Option<UserToken>,
    next_id: u32,
    /// The number of the next batch of downloads we send.
    next_batch: u32,
    out: HashMap<u32, Out>,
    incoming: HashMap<u32, In>,
    /// `(when, bytes done)` per transfer, for the speed.
    speed: HashMap<(bool, u32), (Instant, u64, f64)>,
    last_view: Instant,
    last_log: Instant,
    stopping: bool,
}

impl Worker {
    fn new(rx: Receiver<Cmd>, views: Arc<Mutex<Vec<View>>>, sending: Arc<AtomicBool>) -> Worker {
        Worker {
            rx,
            views,
            sending,
            session: None,
            peer: None,
            folder: None,
            temp: None,
            run_tag: (std::process::id() ^ (crate::cpu::cpu_time(false).as_nanos() as u32))
                .rotate_left(7),
            in_sets: HashMap::new(),
            latest_in: None,
            big_out: HashMap::new(),
            fetches: HashMap::new(),
            blocked: None,
            unreadable: None,
            going_copy: None,
            allowed: true,
            place_fn: None,
            place_checked: None,
            last_refusal: None,
            locked: false,
            impersonating: None,
            next_id: names::first_transfer_id(),
            next_batch: 1,
            out: HashMap::new(),
            incoming: HashMap::new(),
            speed: HashMap::new(),
            last_view: Instant::now(),
            last_log: Instant::now(),
            stopping: false,
        }
    }

    /// Where the files of a new batch of downloads go.
    fn new_batch(&mut self) -> Purpose {
        let batch = self.next_batch;
        self.next_batch = self.next_batch.wrapping_add(1);
        Purpose::Download { batch }
    }

    fn busy(&self) -> bool {
        self.out.values().any(|o| o.state.going())
            || self
                .incoming
                .values()
                .any(|i| i.state == InState::Receiving)
    }

    fn run(mut self) {
        loop {
            // Idle: wait for something to do; busy: look in between rounds.
            let first = if self.busy() {
                match self.rx.try_recv() {
                    Ok(c) => Some(c),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => break,
                }
            } else {
                self.refresh_views(true);
                match self.rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => break,
                }
            };
            if let Some(c) = first {
                self.command(c);
            }
            if self.stopping {
                break;
            }
            if !self.take_commands() {
                return;
            }
            let now = Instant::now();
            self.pump(now);
            self.hash_incoming();
            self.refresh_views(false);
            if self.busy() {
                std::thread::sleep(self.nap(now));
            }
        }
        self.stop();
    }

    /// The commands queued meanwhile; false: the thread is to end (stopped).
    fn take_commands(&mut self) -> bool {
        loop {
            match self.rx.try_recv() {
                Ok(c) => {
                    self.command(c);
                    if self.stopping {
                        self.stop();
                        return false;
                    }
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    self.stop();
                    return false;
                }
            }
        }
    }

    /// How long to sleep between busy rounds: until the pacer or the next
    /// ack is due, at most `NAP`.
    fn nap(&self, now: Instant) -> Duration {
        self.session
            .as_ref()
            .and_then(|s| {
                let a = s.tx.next_poll(now);
                let b = s.rx.next_ack();
                match (a, b) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            })
            .map_or(NAP, |t| t.saturating_duration_since(Instant::now()))
            .min(NAP)
    }

    /// The thread ends: unfinished files are not left behind.
    fn stop(&mut self) {
        self.drop_unfinished();
        self.unimpersonate();
    }

    fn msg(&self, m: FileMsg) {
        if let Some(s) = &self.session {
            (s.io.msg)(m);
        }
    }

    fn event(&self, e: Event) {
        if let Some(s) = &self.session {
            (s.io.event)(e);
        }
    }

    fn command(&mut self, c: Cmd) {
        match c {
            Cmd::Attach {
                session,
                io,
                peer,
                cap_bps,
                place,
                allowed,
            } => {
                self.attach(session, io, peer, cap_bps, place);
                self.policy(allowed, cap_bps);
            }
            Cmd::Policy { allowed, cap_bps } => self.policy(allowed, cap_bps),
            Cmd::Detach(id) => {
                if self.session.as_ref().is_some_and(|s| s.id == id) {
                    self.session = None;
                    self.sending.store(false, Ordering::Relaxed);
                }
            }
            Cmd::Packet(at, p) => self.packet(at, p),
            Cmd::Msg(m) => self.on_msg(m),
            Cmd::Send(_) if !self.allowed => {
                println!("files: not sent: file transfers are off (the admin)");
            }
            Cmd::Send(paths) => {
                let mut files = Vec::new();
                for p in &paths {
                    names::walk(p, "", &mut files, &mut Vec::new());
                }
                let purpose = self.new_batch();
                for (path, name) in files {
                    self.offer_new(path, name, purpose, None);
                }
            }
            Cmd::Copy { set, paths } => self.copy(set, paths),
            Cmd::CopyImage { set, formats } => self.copy_image(set, formats),
            Cmd::Paste(set) => self.paste(set),
            Cmd::Placed(set) => self.msg(FileMsg::Placed { set }),
            Cmd::Cancel { outgoing, transfer } => {
                self.cancel(outgoing, transfer, FileCancel::User, true)
            }
            Cmd::CancelAll => self.cancel_all(true),
            Cmd::Stop => self.stopping = true,
        }
    }

    fn attach(&mut self, id: u32, io: Io, peer: Vec<u8>, cap_bps: u64, place: PlaceFn) {
        if self.peer.as_ref().is_some_and(|p| *p != peer) {
            // Another client: what the last one had going is over.
            self.session = None;
            self.cancel_all(false);
        }
        self.peer = Some(peer);
        let now_place = self.ask_place(&place);
        self.apply_place(now_place);
        self.place_fn = Some(place);
        self.place_checked = Some(Instant::now());
        let now = Instant::now();
        self.session = Some(Session {
            id,
            io,
            tx: BulkSender::new(
                BulkConfig {
                    cap_bps,
                    ..BulkConfig::default()
                },
                now,
            ),
            rx: BulkReceiver::new(),
        });
        self.offer_again();
    }

    /// The admin's switch and cap: off ends what goes at once (pictures go on).
    fn policy(&mut self, allowed: bool, cap_bps: u64) {
        if let Some(s) = self.session.as_mut() {
            s.tx.set_cap(cap_bps);
        }
        if allowed == self.allowed {
            return;
        }
        self.allowed = allowed;
        println!(
            "files: file transfers {} (the admin)",
            if allowed { "on" } else { "off" }
        );
        if allowed {
            // What was copied meanwhile goes now: a paste there needs no new copy.
            if let Some((set, paths)) = self.blocked.take() {
                println!("files: set {set}, copied while transfers were off, goes now");
                self.copy(set, paths);
            }
            return;
        }
        self.big_out.clear();
        let not_pictures = |p: &Purpose| !matches!(p, Purpose::Image { .. });
        let outs: Vec<u32> = self
            .out
            .iter()
            .filter(|(_, o)| not_pictures(&o.purpose))
            .map(|(&id, _)| id)
            .collect();
        for id in outs {
            self.cancel(true, id, FileCancel::NotAllowed, true);
        }
        let ins: Vec<u32> = self
            .incoming
            .iter()
            .filter(|(_, i)| not_pictures(&i.purpose))
            .map(|(&id, _)| id)
            .collect();
        for id in ins {
            self.cancel(false, id, FileCancel::NotAllowed, true);
        }
    }

    fn on_msg(&mut self, m: FileMsg) {
        match m {
            FileMsg::Offer {
                transfer,
                len,
                name,
                purpose,
            } => self.on_offer(transfer, len, name, purpose),
            FileMsg::Copied {
                set,
                files,
                bytes,
                big,
                name,
            } => {
                println!(
                    "files: copied there, set {set}: {name}, {files} files, {bytes} bytes{}",
                    if big { " (on a paste here)" } else { "" }
                );
                // Off: nothing comes; the mark keeps an old paste from
                // pasting, a paste says why.
                let big = big || !self.allowed;
                self.copied_there(set, files, big, false, name);
            }
            FileMsg::CopiedImage { set, formats } => {
                println!("files: picture copied there, set {set}: {formats} formats");
                self.copied_there(set, u32::from(formats), false, true, String::new());
            }
            FileMsg::Fetch { set } => self.fetch(set),
            FileMsg::Refused { set, yours, reason } => self.refused_there(set, yours, reason),
            FileMsg::Placed { set } => self.placed_there(set),
            FileMsg::Accept { transfer, from } => self.on_accept(transfer, from),
            FileMsg::Done { transfer, sha256 } => {
                if let Some(i) = self.incoming.get_mut(&transfer) {
                    i.expect = Some(sha256);
                }
            }
            FileMsg::Saved { transfer } => self.saved_there(transfer),
            FileMsg::Cancel {
                transfer,
                mine,
                reason,
            } => {
                // `mine` from the other side: its outgoing, our incoming.
                self.cancel(!mine, transfer, reason, false);
            }
        }
    }

    /// Ends a transfer; `tell`: the other side is told.
    fn cancel(&mut self, outgoing: bool, id: u32, reason: FileCancel, tell: bool) {
        let now = Instant::now();
        let ended = if outgoing {
            self.cancel_out(id, reason, now)
        } else {
            self.cancel_in(id, reason, now)
        };
        if ended && tell {
            self.msg(FileMsg::Cancel {
                transfer: id,
                mine: outgoing,
                reason,
            });
        }
    }

    fn cancel_all(&mut self, tell: bool) {
        let outs: Vec<u32> = self.out.keys().copied().collect();
        for id in outs {
            self.cancel(true, id, FileCancel::User, tell);
        }
        let ins: Vec<u32> = self.incoming.keys().copied().collect();
        for id in ins {
            self.cancel(false, id, FileCancel::User, tell);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::outgoing::OutState;
    use super::*;

    type Kept<T> = Arc<Mutex<Vec<T>>>;

    /// A worker attached to a session whose messages and events are kept.
    fn attached(dir: &Path) -> (Worker, Kept<FileMsg>, Kept<Event>) {
        let place = dir.to_path_buf();
        attached_with(Box::new(move || Place::Here {
            folder: place.join("down"),
            temp: place.join("temp"),
        }))
    }

    /// The same, with its place given.
    fn attached_with(place: PlaceFn) -> (Worker, Kept<FileMsg>, Kept<Event>) {
        let (_tx, rx) = mpsc::channel();
        let mut w = Worker::new(rx, Default::default(), Default::default());
        let msgs = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let (m, e) = (msgs.clone(), events.clone());
        w.command(Cmd::Attach {
            session: 1,
            io: Io {
                data: Box::new(|_| {}),
                msg: Box::new(move |x| m.lock().unwrap().push(x)),
                event: Box::new(move |x| e.lock().unwrap().push(x)),
            },
            peer: vec![1],
            cap_bps: 100_000_000,
            place,
            allowed: true,
        });
        (w, msgs, events)
    }

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_same_files_copied_again_go_on() {
        let dir = test_dir("again");
        let f = dir.join("a.mov");
        std::fs::write(&f, vec![7u8; 5000]).unwrap();
        let (mut w, msgs, events) = attached(&dir);
        w.command(Cmd::Copy {
            set: 1,
            paths: vec![f.clone()],
        });
        let offered = |m: &[FileMsg]| {
            m.iter()
                .filter(|m| matches!(m, FileMsg::Offer { .. }))
                .count()
        };
        assert_eq!(offered(&msgs.lock().unwrap()), 1);
        // Ctrl+C held: the same files, nothing new goes, nothing stops.
        for set in [2, 3] {
            w.command(Cmd::Copy {
                set,
                paths: vec![f.clone()],
            });
        }
        {
            let m = msgs.lock().unwrap();
            assert_eq!(offered(&m), 1);
            assert!(!m.iter().any(|m| matches!(m, FileMsg::Cancel { .. })));
            assert!(
                !m.iter()
                    .any(|m| matches!(m, FileMsg::Copied { set: 2 | 3, .. }))
            );
        }
        // Placed there: the copies again are placed with it.
        w.command(Cmd::Msg(FileMsg::Placed { set: 1 }));
        assert_eq!(
            *events.lock().unwrap(),
            [Event::Placed(1), Event::Placed(2), Event::Placed(3)]
        );
        // The file changed: it goes again, the old one stops.
        std::fs::write(&f, vec![8u8; 6000]).unwrap();
        w.command(Cmd::Copy {
            set: 4,
            paths: vec![f.clone()],
        });
        {
            let m = msgs.lock().unwrap();
            assert_eq!(offered(&m), 2);
            assert!(m.iter().any(|m| matches!(m, FileMsg::Cancel { .. })));
        }
        // Over (saved there): copied again, it goes again.
        let id = w
            .out
            .iter()
            .find(|(_, o)| o.purpose == Purpose::Clip { set: 4 })
            .map(|(&id, _)| id)
            .unwrap();
        w.command(Cmd::Msg(FileMsg::Saved { transfer: id }));
        w.command(Cmd::Copy {
            set: 5,
            paths: vec![f.clone()],
        });
        assert_eq!(offered(&msgs.lock().unwrap()), 3);
        drop(w);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A place that is the lock screen while `locked` says so.
    fn lockable(dir: &Path, locked: Arc<AtomicBool>) -> PlaceFn {
        let place = dir.to_path_buf();
        Box::new(move || {
            if locked.load(Ordering::Relaxed) {
                Place::Locked
            } else {
                Place::Here {
                    folder: place.join("down"),
                    temp: place.join("temp"),
                }
            }
        })
    }

    /// The owner's host, 2026-10-29: a session opened on the lock screen, the
    /// password typed in it, then a copy: the place was not asked again, the
    /// file was opened as SYSTEM, and the network share refused it.
    #[test]
    fn a_copy_after_a_sign_in_acts_for_the_signed_in_user() {
        let dir = test_dir("signed-in");
        let f = dir.join("a.mov");
        std::fs::write(&f, vec![7u8; 5000]).unwrap();
        let locked = Arc::new(AtomicBool::new(true));
        let (mut w, msgs, _) = attached_with(lockable(&dir, locked.clone()));
        assert!(w.locked);
        locked.store(false, Ordering::Relaxed);
        w.command(Cmd::Copy {
            set: 1,
            paths: vec![f],
        });
        assert!(!w.locked && w.folder.is_some());
        assert!(
            msgs.lock()
                .unwrap()
                .iter()
                .any(|m| matches!(m, FileMsg::Offer { .. }))
        );
        drop(w);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Nobody to act for: the file is not opened at all (it would be as
    /// SYSTEM), and a paste there hears why.
    #[test]
    fn a_copy_on_the_lock_screen_is_not_read() {
        let dir = test_dir("locked-copy");
        let f = dir.join("a.mov");
        std::fs::write(&f, vec![7u8; 5000]).unwrap();
        let locked = Arc::new(AtomicBool::new(true));
        let (mut w, msgs, _) = attached_with(lockable(&dir, locked));
        w.command(Cmd::Copy {
            set: 1,
            paths: vec![f],
        });
        {
            let m = msgs.lock().unwrap();
            assert!(
                matches!(
                    &m[..],
                    [FileMsg::Copied {
                        set: 1,
                        big: true,
                        bytes: 0,
                        ..
                    }]
                ),
                "a mark there, nothing read: {m:?}"
            );
        }
        w.command(Cmd::Msg(FileMsg::Fetch { set: 1 }));
        assert!(msgs.lock().unwrap().iter().any(|m| matches!(
            m,
            FileMsg::Refused {
                set: 1,
                reason: FileCancel::Locked,
                ..
            }
        )));
        drop(w);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_read_refuses_the_paste() {
        let dir = test_dir("unreadable");
        let ok = dir.join("ok.txt");
        std::fs::write(&ok, b"x").unwrap();
        // As the host sees a network drive of the user: no such path.
        let missing = PathBuf::from(r"Z:\no-such-share-lumen\BRW_0030.mov");
        let (mut w, msgs, _) = attached(&dir);
        w.command(Cmd::Copy {
            set: 1,
            paths: vec![ok, missing],
        });
        {
            let m = msgs.lock().unwrap();
            assert!(
                matches!(
                    &m[..],
                    [FileMsg::Copied {
                        set: 1,
                        files: 2,
                        big: true,
                        ..
                    }]
                ),
                "a mark there, nothing offered: {m:?}"
            );
        }
        w.command(Cmd::Msg(FileMsg::Fetch { set: 1 }));
        assert_eq!(
            msgs.lock().unwrap().last(),
            Some(&FileMsg::Refused {
                set: 1,
                yours: false,
                reason: FileCancel::Read
            })
        );
        // A row here too (the client's overlay, when copied there).
        assert!(
            w.out
                .values()
                .any(|o| o.state == OutState::Over(FileCancel::Read))
        );
        drop(w);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

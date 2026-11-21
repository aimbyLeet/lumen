//! File transfers and the clipboard on the network thread: the files thread
//! attached to the session, the host's clipboard sets, pastes here.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use protocol::{FileMsg, FilePacket};
use transport::{PublicKey, ReliableReceiver, Sealer, SeqCounter, UdpLink};

use super::checks::TestPastes;

/// What the network thread needs for file transfers: the service (it lives
/// as long as the program), where received files go.
#[derive(Clone)]
pub struct FilesHook {
    pub commands: Sender<desktop::files::Cmd>,
    pub folder: std::path::PathBuf,
    /// Where the host's clipboard sets go.
    pub temp: std::path::PathBuf,
    /// `--file-mbps`, over the server's cap.
    pub cap_mbps: Option<f64>,
    /// What the session may do with files, as the server says now.
    pub policy: Arc<Mutex<signal::FilePolicy>>,
    /// `--send-file`: sent once, with the first session.
    pub send_first: Arc<Mutex<Vec<std::path::PathBuf>>>,
}

impl FilesHook {
    /// The cap of our file data, bit/s: `--file-mbps`, else the server's.
    pub fn cap_bps(&self, policy: signal::FilePolicy) -> u64 {
        policy.cap_bps(self.cap_mbps)
    }
}

/// The files thread, attached to this session.
pub(super) struct AttachedFiles {
    pub(super) commands: Sender<desktop::files::Cmd>,
    /// Its messages for the host, its word to the clipboard.
    pub(super) msgs: Receiver<FileMsg>,
    pub(super) events: Receiver<desktop::files::Event>,
    /// The server's word on files when it was attached.
    pub(super) policy: signal::FilePolicy,
}

/// Attaches the files thread to the session: its data sealed out on a copy
/// of `link`; `--send-file` goes with the first session.
pub(super) fn attach_files(
    hook: &FilesHook,
    link: &UdpLink,
    session: u32,
    seq: &SeqCounter,
    sealer: &Arc<Sealer>,
    host_key: &PublicKey,
) -> AttachedFiles {
    let (msg_tx, msgs) = mpsc::channel();
    let (event_tx, events) = mpsc::channel();
    let policy = *hook.policy.lock().unwrap();
    match link.try_clone() {
        Ok(file_link) => {
            let (seq, sealer) = (seq.clone(), sealer.clone());
            let (folder, temp) = (hook.folder.clone(), hook.temp.clone());
            let _ = hook.commands.send(desktop::files::Cmd::Attach {
                session,
                io: desktop::files::Io {
                    data: Box::new(move |p: &FilePacket| {
                        let mut d = p.datagram(session, seq.take());
                        sealer
                            .seal(&mut d)
                            .expect("a datagram we built has a header");
                        let _ = file_link.send(&d);
                    }),
                    msg: Box::new(move |m| {
                        let _ = msg_tx.send(m);
                    }),
                    event: Box::new(move |e| {
                        let _ = event_tx.send(e);
                    }),
                },
                peer: host_key.0.to_vec(),
                cap_bps: hook.cap_bps(policy),
                allowed: policy.allowed,
                place: Box::new(move || desktop::files::Place::Here {
                    folder: folder.clone(),
                    temp: temp.clone(),
                }),
            });
        }
        Err(e) => eprintln!("files: {e}"),
    }
    let first = std::mem::take(&mut *hook.send_first.lock().unwrap());
    if !first.is_empty() {
        let _ = hook.commands.send(desktop::files::Cmd::Send(first));
    }
    AttachedFiles {
        commands: hook.commands.clone(),
        msgs,
        events,
        policy,
    }
}

/// The network thread's files and clipboard.
pub(super) struct Sharing {
    pub(super) clipboard: Option<crate::clipboard::ClientClipboard>,
    /// The files thread, its messages for the host (they go in the input
    /// stream), and the host's to it (reliable, in order).
    pub(super) files: Option<Sender<desktop::files::Cmd>>,
    pub(super) file_msgs: Option<Receiver<FileMsg>>,
    pub(super) file_in: ReliableReceiver<FileMsg>,
    /// What the files thread tells the clipboard.
    pub(super) file_events: Option<Receiver<desktop::files::Event>>,
    /// The server's word on files, and what the files thread was told last.
    pub(super) file_policy: Option<(FilesHook, signal::FilePolicy)>,
    /// Pastes in other programs seen so far (`shortcuts::pastes`).
    pub(super) pastes: u64,
    pub(super) hold: crate::paste::Hold,
}

impl Sharing {
    /// The host's clipboard sets onto ours, ours placed there, and pastes
    /// here that fetch a big set of the host's.
    pub(super) fn clipboard_sets(&mut self, tests: &mut TestPastes) {
        if let (Some((hook, told)), Some(files)) = (self.file_policy.as_mut(), self.files.as_ref())
        {
            let now = *hook.policy.lock().unwrap();
            if now != *told {
                *told = now;
                let _ = files.send(desktop::files::Cmd::Policy {
                    allowed: now.allowed,
                    cap_bps: hook.cap_bps(now),
                });
            }
        }
        let (Some(c), Some(files)) = (self.clipboard.as_mut(), self.files.as_ref()) else {
            return;
        };
        if let Some(events) = &self.file_events {
            for ev in events.try_iter() {
                if let desktop::files::Event::Placed(set) = ev {
                    self.hold.placed(set);
                } else if let Some(set) = c.on_files_event(ev) {
                    let _ = files.send(desktop::files::Cmd::Placed(set));
                }
            }
        }
        let now = Instant::now();
        let pastes = crate::shortcuts::pastes();
        let test = tests.test_paste.last().is_some_and(|&t| now >= t);
        if pastes != self.pastes || test {
            self.pastes = pastes;
            if test {
                tests.test_paste.pop();
                println!("clipboard: test: a paste here");
            }
            if let Some(set) = c.pending() {
                let _ = files.send(desktop::files::Cmd::Paste(set));
            }
        }
        if tests.test_host_paste.last().is_some_and(|&t| now >= t) {
            tests.test_host_paste.pop();
            println!("clipboard: test: Ctrl+V in the video window");
            self.hold.extend(crate::paste::ctrl_v());
        }
    }
}

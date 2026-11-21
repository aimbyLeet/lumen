//! Client input on the host: reliable, ordered reception and application.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use input::Injector;
use protocol::InputEvent;
use transport::ReliableReceiver;

use crate::clipboard::HostClipboard;

/// How often, at most, the input thread checks which desktop takes input.
const DESKTOP_CHECK_EVERY: Duration = Duration::from_millis(50);

/// Ctrl+Alt+Del: only the service may make it (SendSAS); its hosts ask it.
/// Not on this thread: the input after it is not held up.
fn secure_attention() {
    std::thread::spawn(|| {
        use desktop::hostsvc::{Request, call};
        match call(&Request::SecureAttention) {
            Ok(_) => println!("input: Ctrl+Alt+Del"),
            Err(e) => eprintln!("input: Ctrl+Alt+Del not sent: {e}"),
        }
    });
}

/// What happens to input events once they are in order.
pub enum Sink {
    /// Only logged (and counted): host and client share one desktop, injecting
    /// would click into the client's own window.
    DryRun,
    Inject(Injector),
    /// Input is refused (not even logged).
    Off,
}

pub struct RemoteInput {
    receiver: ReliableReceiver<InputEvent>,
    sink: Sink,
    log: Option<BufWriter<File>>,
    pub applied: u64,
    pub errors: u64,
    /// When this thread last made sure it is on the input desktop.
    desktop_checked: Option<Instant>,
    /// The client's clipboard texts come in the input stream; `None`: off.
    pub clipboard: Option<HostClipboard>,
    /// File transfer messages from the client, to the files thread.
    pub files: Option<std::sync::mpsc::Sender<desktop::files::Cmd>>,
}

impl RemoteInput {
    pub fn new(sink: Sink, log: Option<&Path>, clipboard: Option<HostClipboard>) -> Result<Self> {
        let log = match log {
            Some(p) => Some(crate::files::create_log(p)?),
            None => None,
        };
        Ok(RemoteInput {
            receiver: ReliableReceiver::new(),
            sink,
            log,
            applied: 0,
            errors: 0,
            desktop_checked: None,
            clipboard,
            files: None,
        })
    }

    /// `SendInput` reaches the thread's desktop only: the lock screen or a UAC
    /// prompt is another one (reachable when running as SYSTEM).
    fn follow_input_desktop(&mut self) {
        if self
            .desktop_checked
            .is_some_and(|t| t.elapsed() < DESKTOP_CHECK_EVERY)
        {
            return;
        }
        self.desktop_checked = Some(Instant::now());
        match capture::follow_input_desktop() {
            Ok(Some(name)) => println!("input: now on the {name} desktop"),
            Ok(None) => {}
            Err(e) => eprintln!("input: {e:#}"),
        }
    }

    /// Injected over the whole desktop: the client's pointer positions are
    /// desktop coordinates (protocol v10), on whichever monitor.
    pub fn injector() -> Sink {
        Sink::Inject(Injector::for_desktop())
    }

    /// Takes input event `id`; applies everything now in order. Returns the
    /// cumulative acknowledgement to send back.
    pub fn accept(&mut self, id: u32, event: InputEvent) -> Option<u32> {
        if matches!(self.sink, Sink::Off) {
            return None;
        }
        let first = self
            .receiver
            .last_delivered()
            .map_or(0, |l| l.wrapping_add(1));
        for (k, ev) in self.receiver.accept(id, event).into_iter().enumerate() {
            if let Some(log) = self.log.as_mut() {
                let _ = writeln!(log, "{}\t{ev:?}", first.wrapping_add(k as u32));
            }
            // Onto the clipboard before the next event (a Ctrl+V) is applied.
            if let InputEvent::Clipboard(piece) = ev {
                if let Some(c) = self.clipboard.as_mut() {
                    c.on_piece(piece);
                }
                self.applied += 1;
                continue;
            }
            if let InputEvent::File(m) = ev {
                if let Some(f) = &self.files {
                    let _ = f.send(desktop::files::Cmd::Msg(m));
                }
                self.applied += 1;
                continue;
            }
            if let Some(c) = self.clipboard.as_mut() {
                c.on_input(&ev);
            }
            if let Sink::Inject(_) = self.sink {
                self.follow_input_desktop();
                if ev == InputEvent::SecureAttention {
                    secure_attention();
                }
            }
            self.inject_following_desktop(&ev);
            self.applied += 1;
        }
        self.receiver.last_delivered()
    }

    /// Applies `ev`; counts (and says, the first few) the failures.
    fn inject_following_desktop(&mut self, ev: &InputEvent) {
        let mut result = self.inject(ev);
        if result.is_err() {
            // Refused (access denied): the input desktop changed since the
            // last look, e.g. the lock screen came up. Follow it now, once more.
            self.desktop_checked = None;
            self.follow_input_desktop();
            result = self.inject(ev);
        }
        if let Err(e) = result {
            self.errors += 1;
            if self.errors <= 5 {
                eprintln!("input: {e:#}");
            }
        }
    }

    fn inject(&mut self, ev: &InputEvent) -> Result<()> {
        match &mut self.sink {
            Sink::Inject(inj) => inj.apply(ev),
            Sink::DryRun | Sink::Off => Ok(()),
        }
    }

    /// Session over: release every key/button still held, flush the log.
    pub fn finish(&mut self) {
        if let Sink::Inject(inj) = &mut self.sink {
            let _ = inj.release_all();
        }
        if let Some(log) = self.log.as_mut() {
            let _ = log.flush();
        }
        if let Some(c) = self.clipboard.as_mut() {
            c.finish();
        }
    }

    pub fn duplicates(&self) -> u64 {
        self.receiver.duplicates()
    }
}

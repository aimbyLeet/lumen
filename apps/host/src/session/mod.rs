//! One streaming session with an admitted client: a stream per monitor
//! (`streamer`: capture → P010 → NVENC → FEC) into one paced UDP sender,
//! steered by what the client sends (control thread events) and by the server
//! (the session may end there).

mod monitors;
mod pacing;
mod rate;
mod report;
mod start;

use std::fs::File;
use std::io::BufWriter;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use protocol::ControlMessage;
use signal::{HostAgent, IncomingSession};
use transport::{PacedSender, Packetizer, UdpLink, wall_clock_us};

use self::monitors::Monitors;
use self::pacing::Pace;
use self::rate::Ceiling;
use self::report::report_audio;
use crate::audio::AudioThread;
use crate::control::{Event, control_datagram};
use crate::remote_input::RemoteInput;
use crate::station::{Admitted, End, Station};
use crate::streamer::{Cmd, Shared, Stream, send_cursor_shape};

/// `Monitors` goes again this often until the client answers it.
const MONITORS_AGAIN: Duration = Duration::from_millis(250);
/// How often the host looks whether its monitors changed.
const LAYOUT_EVERY: Duration = Duration::from_secs(1);
/// The main loop waits for the client's events at most this long.
const EVENT_WAIT: Duration = Duration::from_millis(5);
/// A session stops when the client sends nothing for this long (it sends
/// stats every second; silence means it crashed or lost its network).
const CLIENT_SILENCE: Duration = Duration::from_secs(10);

/// One session with an admitted client over `link` (connected to it), until it ends.
pub fn run(
    st: &mut Station,
    link: UdpLink,
    adm: Admitted,
    agent: &HostAgent,
    inc: &IncomingSession,
) -> Result<End> {
    let started = (Instant::now(), desktop::cpu::cpu_time(false));
    let mut s = Session::start(st, link, adm, agent, inc)?;
    let end = s.stream();
    s.finish(&end);
    let secs = started.0.elapsed().as_secs_f64();
    let cpu = (desktop::cpu::cpu_time(false) - started.1).as_secs_f64();
    println!(
        "cpu: {cpu:.2} s in {secs:.1} s ({:.1} % of a core)",
        100.0 * cpu / secs
    );
    end
}

struct Session<'a> {
    st: &'a mut Station,
    agent: &'a HostAgent,
    inc: &'a IncomingSession,
    /// The session id in our datagrams' headers.
    id: u32,
    events: Receiver<Event>,
    control: Option<JoinHandle<(RemoteInput, u64)>>,
    stop: Arc<AtomicBool>,
    /// When the client was last heard from (`wall_clock_us`, the control thread's).
    last_rx: Arc<AtomicU64>,
    /// One per monitor, in the order of `Monitors`.
    streams: Vec<Stream>,
    /// What the client decodes (the streams' formats are checked against it).
    caps: protocol::Capabilities,
    shared: Shared,
    fps: u32,
    /// The encoder's rate control and the session's ceiling.
    ceiling: Ceiling,
    /// Pacing and FEC parity, as the client asks.
    pace: Pace,
    /// Cursor shapes the client asks for again.
    cursor_packetizer: Packetizer,
    /// Before `sender`: it sends through it.
    audio: Option<AudioThread>,
    sender: PacedSender,
    /// `--dump-stream`: each monitor's stream until it first starts (monitor
    /// 1 to the path, monitor `n` + 1 to `<path>.<n>`).
    dumps: Vec<Option<BufWriter<File>>>,
    win_start: Instant,
    deadline: Option<Instant>,
    /// The monitors told to the client, the ones it shows, and the question
    /// when there is none.
    monitors: Monitors,
}

impl Session<'_> {
    /// Stops the streams and the control thread. Bye only when the host itself
    /// ends the stream (the client then stops): when the server ended it, or
    /// it failed, the client hears from the server and may reconnect.
    fn finish(&mut self, end: &Result<End>) {
        self.stop_streams();
        if matches!(end, Ok(e) if *e != End::Server) {
            self.send_control(&ControlMessage::Bye);
        }
        // Closed by the client: its transfers are over; lost: it may come
        // back and go on with them.
        if matches!(end, Ok(End::Bye)) {
            let _ = self
                .st
                .files
                .commands()
                .send(desktop::files::Cmd::CancelAll);
        }
        let _ = self
            .st
            .files
            .commands()
            .send(desktop::files::Cmd::Detach(self.id));
        thread::sleep(Duration::from_millis(50));
        if let Some(a) = self.audio.take().and_then(AudioThread::stop) {
            report_audio(&a);
        }
        self.stop.store(true, Ordering::Relaxed);
        let Some(control) = self.control.take() else {
            return;
        };
        let (input, rejected) = control.join().expect("control thread");
        println!(
            "input: {} events applied, {} duplicates dropped, {} injection errors; {} datagrams failed decryption",
            input.applied,
            input.duplicates(),
            input.errors,
            rejected
        );
    }

    fn send_control(&self, msg: &ControlMessage) {
        self.sender.send_now(control_datagram(
            &self.shared.seq,
            self.id,
            &self.shared.sealer,
            msg,
        ));
    }

    /// The streams run on their own threads; this one listens to the client,
    /// the server and the desktop until one of them ends the session.
    fn stream(&mut self) -> Result<End> {
        loop {
            if let Some(end) = self.st.server_says(self.agent, self.inc) {
                return Ok(end);
            }
            if let Some(end) = self.client_events()? {
                return Ok(end);
            }
            if let Some(end) = self.ends()? {
                return Ok(end);
            }
            self.tell_again();
            self.steer_files();
            self.follow_choice()?;
            self.look_at_layout()?;
            self.report_each_second();
        }
    }

    /// What the client sent meanwhile (waits for it a little); why the session
    /// ends, if it does.
    fn client_events(&mut self) -> Result<Option<End>> {
        match self.events.recv_timeout(EVENT_WAIT) {
            Ok(ev) => {
                if let Some(end) = self.on_event(ev)? {
                    return Ok(Some(end));
                }
                while let Ok(ev) = self.events.try_recv() {
                    if let Some(end) = self.on_event(ev)? {
                        return Ok(Some(end));
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("control thread ended"),
        }
        Ok(None)
    }

    /// The deadline, the client's silence, a stream that ended.
    fn ends(&mut self) -> Result<Option<End>> {
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(Some(End::Deadline));
        }
        let silent_us = wall_clock_us().saturating_sub(self.last_rx.load(Ordering::Relaxed));
        if silent_us > CLIENT_SILENCE.as_micros() as u64 {
            println!("nothing from the client for {CLIENT_SILENCE:?}");
            return Ok(Some(End::Silent));
        }
        if let Some(i) = self.streams.iter().position(Stream::ended) {
            // Its screen is not kept, and the stream stays in the list:
            // `stop_streams` then finds a screen missing and gives none back,
            // so the next session opens them all anew (with the others' kept,
            // it would have streamed a monitor less).
            let r = self.streams[i].join().map(|(_, r, _)| r);
            return Err(match r {
                Some(Err(e)) => e.context(format!("monitor {}", i + 1)),
                _ => anyhow!("monitor {}: its stream ended", i + 1),
            });
        }
        Ok(None)
    }

    /// `Monitors` again until the client answers it; the waiting message when due.
    fn tell_again(&mut self) {
        if !self.monitors.monitors_acked && self.monitors.monitors_sent.elapsed() >= MONITORS_AGAIN
        {
            self.send_control(&self.monitors.monitors_msg);
            self.monitors.monitors_sent = Instant::now();
        }
        if self.monitors.asking.due() {
            self.send_waiting();
        }
    }

    /// The monitors looked at every `LAYOUT_EVERY`.
    fn look_at_layout(&mut self) -> Result<()> {
        // While the service makes the client's monitor, the layout is
        // half done: looked at once it is.
        if self.monitors.choosing.is_none()
            && self.monitors.layout_checked.elapsed() >= LAYOUT_EVERY
        {
            self.monitors.layout_checked = Instant::now();
            self.follow_layout()?;
        }
        Ok(())
    }

    /// The last second's report, once a second.
    fn report_each_second(&mut self) {
        if self.win_start.elapsed() >= Duration::from_secs(1) {
            self.report();
        }
    }

    /// What the control thread passed on from the client; why the session ends, if it does.
    fn on_event(&mut self, ev: Event) -> Result<Option<End>> {
        match ev {
            Event::Hello => {
                // Client restarted: the monitors and formats again, every stream from an IDR.
                self.send_monitors();
                self.monitors.told_again();
                self.streams.iter().for_each(|s| s.send(Cmd::Idr));
            }
            Event::Keyframe { stream, frame } => {
                if let Some(s) = self.streams.get(usize::from(stream)) {
                    s.send(Cmd::Keyframe { frame });
                }
            }
            Event::Recovery {
                stream,
                last_good,
                lost,
            } => {
                if let Some(s) = self.streams.get(usize::from(stream)) {
                    s.send(Cmd::Recovery { last_good, lost });
                }
            }
            Event::Show {
                generation,
                streams,
            } if generation == self.st.generation => {
                self.monitors.monitors_acked = true;
                if streams != self.monitors.shown {
                    self.monitors.shown = streams;
                    for (i, s) in self.streams.iter().enumerate() {
                        s.send(Cmd::Show(streams & (1 << i) != 0));
                    }
                    self.follow_shown();
                }
            }
            // About monitors that are gone.
            Event::Show { .. } => {}
            Event::Stats(s) => println!(
                "client: {} pkts, {} lost, {} FEC-restored, {} decoded, {} dropped, {} keyframe / {} recovery requests",
                s.packets_received,
                s.packets_lost,
                s.fec_recovered,
                s.frames_decoded,
                s.frames_dropped,
                s.keyframes_requested,
                s.recoveries_requested
            ),
            Event::CursorShape { id } => {
                let shape = self.shared.cursor.lock().unwrap().shape(id).cloned();
                if let Some(shape) = shape {
                    send_cursor_shape(
                        &mut self.cursor_packetizer,
                        &self.shared.out,
                        &self.shared.sealer,
                        &shape,
                    )?;
                }
            }
            Event::Pacing {
                rate_kbps,
                burst_bytes,
            } => self.set_pacing(rate_kbps, burst_bytes),
            Event::Fec { parity_permille } => self.set_fec(parity_permille),
            Event::MonitorChoice {
                ask,
                width,
                height,
                scale,
            } => self.on_choice(ask, width, height, scale)?,
            Event::Bye => {
                println!("client said bye");
                return Ok(Some(End::Bye));
            }
        }
        Ok(None)
    }
}

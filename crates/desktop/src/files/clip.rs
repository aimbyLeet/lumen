//! Clipboard sets: files and pictures copied on one side, onto the other
//! side's clipboard — small ones at once, big ones on a paste there.

use std::fs::File;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use protocol::{FileCancel, FileMsg, Purpose};

use super::incoming::In;
use super::names::{safe_name, top_of, walk};
use super::outgoing::Out;
use super::{CLIP_LIMIT, Event, REFUSAL_EVERY, Worker};

/// Our small clipboard set on its way (see `Worker::going_copy`).
pub(super) struct GoingCopy {
    set: u32,
    files: Vec<FileId>,
    /// Sets copied since with the same files: placed when `set` is.
    again: Vec<u32>,
}

/// What a copied file is: path, length, modified.
type FileId = (PathBuf, u64, Option<SystemTime>);

/// The other side's clipboard set as it comes.
pub(super) struct InSet {
    expected: u32,
    done: u32,
    big: bool,
    image: bool,
    /// What came: its purpose, name and where it is.
    parts: Vec<(Purpose, String, PathBuf)>,
    /// What was copied first (a refused paste names it).
    name: String,
}

/// The files of a copy, opened now: one that cannot be read (a network
/// drive the host does not see, no right to it) refuses a paste there — not
/// a set that waits for it forever. Small sets go from these handles.
struct Opened {
    files: Vec<(PathBuf, String)>,
    handles: Vec<(File, u64, Option<SystemTime>)>,
    /// Names of what could not be read or listed.
    unreadable: Vec<String>,
}

impl Opened {
    fn of(paths: &[PathBuf]) -> Opened {
        let mut files = Vec::new();
        let mut unreadable = Vec::new();
        for p in paths {
            walk(p, "", &mut files, &mut unreadable);
        }
        let mut handles = Vec::with_capacity(files.len());
        for (path, name) in &files {
            match File::open(path).and_then(|f| f.metadata().map(|m| (f, m))) {
                Ok((f, m)) => handles.push((f, m.len(), m.modified().ok())),
                Err(e) => {
                    eprintln!("files: {}: {e}", path.display());
                    unreadable.push(name.clone());
                }
            }
        }
        Opened {
            files,
            handles,
            unreadable,
        }
    }

    /// Nobody to read them for: not touched, all unreadable.
    fn not_read(paths: &[PathBuf]) -> Opened {
        let files: Vec<(PathBuf, String)> = paths
            .iter()
            .map(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                (p.clone(), name.into_owned())
            })
            .collect();
        Opened {
            unreadable: files.iter().map(|(_, name)| name.clone()).collect(),
            files,
            handles: Vec::new(),
        }
    }

    fn readable(&self) -> bool {
        self.unreadable.is_empty()
    }

    fn bytes(&self) -> u64 {
        self.handles.iter().map(|o| o.1).sum()
    }

    /// What the files are (all opened when it is compared).
    fn ids(&self) -> Vec<FileId> {
        self.files
            .iter()
            .zip(&self.handles)
            .map(|((p, _), (_, len, t))| (p.clone(), *len, *t))
            .collect()
    }
}

impl Worker {
    /// The folder of the other side's clipboard set `set`.
    pub(super) fn clip_dir(&self, set: u32) -> Option<PathBuf> {
        let t = self.temp.as_ref()?;
        Some(
            t.join("Lumen")
                .join("clip")
                .join(format!("{:08x}-{set}", self.run_tag)),
        )
    }

    /// A new set copied here: what is still going of the older ones stops.
    fn drop_old_sets(&mut self, set: u32) {
        let old: Vec<u32> = self
            .out
            .iter()
            .filter(|(_, o)| {
                matches!(o.purpose, Purpose::Clip { set: s } | Purpose::Image { set: s, .. } if s != set)
                    && o.state.going()
            })
            .map(|(&id, _)| id)
            .collect();
        for id in old {
            self.cancel(true, id, FileCancel::User, true);
        }
        self.big_out.clear();
        self.fetches.clear();
        self.blocked = None;
        self.unreadable = None;
        self.going_copy = None;
    }

    /// Transfers of our set `set` still go.
    fn set_going(&self, set: u32) -> bool {
        self.out
            .values()
            .any(|o| matches!(o.purpose, Purpose::Clip { set: s } if s == set) && o.state.going())
    }

    /// A paste of files that cannot go (`reason`): a row that says so (the
    /// overlay), nothing else. `outgoing`: copied here, pasted there.
    fn refused_paste(&mut self, name: String, outgoing: bool, reason: FileCancel) {
        println!("files: pasted {name}, but it cannot go: {reason:?}");
        let now = Instant::now();
        // Numbered apart from the transfers.
        if outgoing {
            let id = u32::MAX - self.out.len() as u32;
            self.out.insert(id, Out::refused(name, reason, now));
        } else {
            let id = u32::MAX - self.incoming.len() as u32;
            self.incoming.insert(id, In::refused(name, reason, now));
        }
        self.refresh_views(true);
    }

    /// The same files copied again while they still go (Ctrl+C held, pressed
    /// twice): what goes on goes, nothing starts again.
    fn copied_again(&mut self, set: u32, ids: &[FileId]) -> bool {
        let Some(first) = self
            .going_copy
            .as_ref()
            .filter(|g| g.files == ids && self.set_going(g.set))
            .map(|g| g.set)
        else {
            return false;
        };
        println!("files: set {set}: the files of set {first} again, which goes on");
        if let Some(g) = self.going_copy.as_mut() {
            g.again.push(set);
        }
        true
    }

    pub(super) fn copy(&mut self, set: u32, paths: Vec<PathBuf>) {
        // Read as whoever is signed in now, never as SYSTEM.
        self.refresh_place_now();
        let opened = if self.folder.is_some() {
            Opened::of(&paths)
        } else {
            Opened::not_read(&paths)
        };
        let ids = opened.ids();
        if opened.readable() && self.allowed && self.copied_again(set, &ids) {
            return;
        }
        self.drop_old_sets(set);
        let bytes = opened.bytes();
        // Over a limit, or something that cannot go: a mark there, a paste
        // there says the rest.
        let big = bytes > CLIP_LIMIT || !opened.readable();
        let name = paths
            .first()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!(
            "files: copied here, set {set}: {} files, {bytes} bytes{}{}",
            opened.files.len(),
            if bytes > CLIP_LIMIT {
                " (sent on a paste there)"
            } else {
                ""
            },
            if opened.readable() {
                String::new()
            } else {
                let actor = if self.folder.is_none() {
                    "not read: nobody to act for"
                } else if self.impersonating.is_some() {
                    "as the signed-in user"
                } else {
                    "as this process"
                };
                format!(", cannot read {} ({actor})", opened.unreadable.join(", "))
            }
        );
        self.msg(FileMsg::Copied {
            set,
            files: opened.files.len() as u32,
            bytes,
            big,
            name,
        });
        if !self.allowed {
            // Only that it was copied: a paste there says transfers are off
            // (the set is kept for its name).
            println!("files: set {set} stays here: file transfers are off (the admin)");
            self.big_out.insert(set, opened.files);
            self.blocked = Some((set, paths));
        } else if !opened.readable() {
            // Kept for its name: a paste there gets a row that says why.
            self.big_out.insert(set, opened.files);
            self.unreadable = Some(set);
        } else if big {
            self.big_out.insert(set, opened.files);
        } else {
            self.going_copy = Some(GoingCopy {
                set,
                files: ids,
                again: Vec::new(),
            });
            for ((path, name), (file, _, _)) in opened.files.into_iter().zip(opened.handles) {
                self.offer_new(path, name, Purpose::Clip { set }, Some(file));
            }
        }
    }

    pub(super) fn copy_image(&mut self, set: u32, formats: Vec<(u8, Vec<u8>)>) {
        self.drop_old_sets(set);
        let Some(temp) = self.temp.clone() else {
            return;
        };
        let dir = temp
            .join("Lumen")
            .join("clip-out")
            .join(format!("{:08x}-{set}", self.run_tag));
        let mut written = Vec::new();
        for (format, bytes) in &formats {
            let path = dir.join(format!("image-{format}"));
            match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes)) {
                Ok(()) => written.push((*format, path)),
                Err(e) => eprintln!("files: {}: {e}", path.display()),
            }
        }
        println!(
            "files: picture copied here, set {set}: {} formats, {} bytes",
            written.len(),
            formats.iter().map(|(_, b)| b.len()).sum::<usize>()
        );
        self.msg(FileMsg::CopiedImage {
            set,
            formats: written.len() as u8,
        });
        for (format, path) in written {
            self.offer_new(
                path,
                format!("image-{format}"),
                Purpose::Image { set, format },
                None,
            );
        }
    }

    /// The other side copied set `set`: `expected` transfers make it.
    pub(super) fn copied_there(
        &mut self,
        set: u32,
        expected: u32,
        big: bool,
        image: bool,
        name: String,
    ) {
        self.latest_in = Some(set);
        self.in_sets.insert(
            set,
            InSet {
                expected,
                done: 0,
                big,
                image,
                parts: Vec::new(),
                name,
            },
        );
        self.event(Event::Pending { set, big });
        if !big && expected == 0 {
            self.finish_set(set);
        }
    }

    /// The other side put our set `set` on its clipboard; the sets copied
    /// again meanwhile are there too.
    pub(super) fn placed_there(&mut self, set: u32) {
        self.event(Event::Placed(set));
        let again = self
            .going_copy
            .as_ref()
            .filter(|g| g.set == set)
            .map(|g| g.again.clone())
            .unwrap_or_default();
        for s in again {
            self.event(Event::Placed(s));
        }
    }

    /// A transfer of set `set` saved at `path`.
    pub(super) fn set_part(&mut self, set: u32, purpose: Purpose, name: String, path: PathBuf) {
        let Some(s) = self.in_sets.get_mut(&set) else {
            return;
        };
        s.done += 1;
        s.parts.push((purpose, name, path));
        if s.done >= s.expected {
            self.finish_set(set);
        }
    }

    /// The whole set is here: onto the clipboard, unless a newer one came.
    fn finish_set(&mut self, set: u32) {
        let Some(s) = self.in_sets.remove(&set) else {
            return;
        };
        if self.latest_in != Some(set) {
            return;
        }
        if s.image {
            println!("files: picture of set {set} is here: onto the clipboard");
            self.event(Event::Image(set, image_formats(&s.parts)));
            return;
        }
        let Some(dir) = self.clip_dir(set) else {
            return;
        };
        // What was copied: the first part of each name (a file or a folder).
        let mut tops: Vec<PathBuf> = Vec::new();
        for (_, name, _) in &s.parts {
            if let Some(first) = safe_name(name).components().next() {
                let p = dir.join(first.as_os_str());
                if !tops.contains(&p) {
                    tops.push(p);
                }
            }
        }
        println!(
            "files: set {set} is here ({} files): onto the clipboard",
            s.parts.len()
        );
        self.event(Event::Files(set, tops));
    }

    /// A paste here while the mark of the other side's set `set` was on the
    /// clipboard: a big set is fetched, again on each paste once the last
    /// one's transfers are over (the sender sees to that).
    pub(super) fn paste(&mut self, set: u32) {
        let Some(s) = self.in_sets.get(&set) else {
            return;
        };
        if !s.big {
            return;
        }
        let name = s.name.clone();
        let refusal = if !self.allowed {
            Some(FileCancel::NotAllowed)
        } else {
            self.unavailable()
        };
        let Some(reason) = refusal else {
            println!("files: pasted here: fetching set {set}");
            self.msg(FileMsg::Fetch { set });
            return;
        };
        if !self.refusal_due() {
            return;
        }
        self.refused_paste(name, false, reason);
        // The other side shows it too (the overlay is the client's).
        self.msg(FileMsg::Refused {
            set,
            yours: true,
            reason,
        });
    }

    /// A refused paste gets a row (the overlay comes up with it) once per
    /// `REFUSAL_EVERY`: each paste, not each key repeat of a held Ctrl+V.
    fn refusal_due(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_refusal
            .is_some_and(|t| now.duration_since(t) < REFUSAL_EVERY)
        {
            return false;
        }
        self.last_refusal = Some(now);
        true
    }

    /// The other side pasted our big set `set`: it goes, unless what the
    /// last paste started still goes.
    pub(super) fn fetch(&mut self, set: u32) {
        let Some(files) = self.big_out.get(&set).cloned() else {
            println!("files: set {set} asked for, but it is not the latest");
            return;
        };
        let refusal = if !self.allowed {
            Some(FileCancel::NotAllowed)
        } else {
            self.unavailable()
                .or((self.unreadable == Some(set)).then_some(FileCancel::Read))
        };
        if let Some(reason) = refusal {
            if !self.refusal_due() {
                return;
            }
            if let Some((_, name)) = files.first() {
                self.refused_paste(top_of(name), true, reason);
            }
            self.msg(FileMsg::Refused {
                set,
                yours: false,
                reason,
            });
            return;
        }
        let going = self.fetches.get(&set).is_some_and(|ids| {
            ids.iter()
                .any(|id| self.out.get(id).is_some_and(|o| o.state.going()))
        });
        if going {
            println!("files: set {set} pasted again while it goes: nothing new");
            return;
        }
        println!("files: pasted there: sending set {set}");
        let purpose = self.new_batch();
        let ids = files
            .into_iter()
            .filter_map(|(path, name)| self.offer_new(path, name, purpose, None))
            .collect();
        self.fetches.insert(set, ids);
    }

    /// The other side could not start a paste of set `set`: the row here.
    pub(super) fn refused_there(&mut self, set: u32, yours: bool, reason: FileCancel) {
        let name = if yours {
            self.big_out
                .get(&set)
                .and_then(|f| f.first())
                .map(|(_, n)| top_of(n))
        } else {
            self.in_sets.get(&set).map(|s| s.name.clone())
        };
        if let Some(name) = name {
            self.refused_paste(name, yours, reason);
        }
    }
}

/// A picture's formats as they came (read back from their files).
fn image_formats(parts: &[(Purpose, String, PathBuf)]) -> Vec<(u8, Vec<u8>)> {
    let mut formats = Vec::new();
    for (purpose, _, path) in parts {
        if let Purpose::Image { format, .. } = purpose {
            match std::fs::read(path) {
                Ok(b) => formats.push((*format, b)),
                Err(e) => eprintln!("files: {}: {e}", path.display()),
            }
        }
    }
    formats
}

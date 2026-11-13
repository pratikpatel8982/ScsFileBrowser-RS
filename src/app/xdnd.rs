//! Drag source for X11 (the XDND protocol), used to drag files out of the window on
//! Linux/BSD, where the `drag` crate can't attach to eframe's winit window.
//!
//! winit keeps the implicit pointer grab while the mouse button is held, so this does
//! not grab anything itself. A second X connection polls the pointer, finds the
//! XDND-aware window under it, and speaks the protocol to it:
//!
//! `XdndEnter` -> `XdndPosition` (repeated) <- `XdndStatus`, then on button release
//! `XdndDrop`; the target asks for the `XdndSelection` as `text/uri-list`, which is
//! answered with `file://` URIs, and finally sends `XdndFinished`.
//!
//! The session runs on its own thread so the UI stays responsive.

use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, KeyButMask,
    PropMode, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
    CLIENT_MESSAGE_EVENT, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

type Res<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// XDND protocol version we speak.
const VERSION: u32 = 5;
const POLL: Duration = Duration::from_millis(8);
/// How long to wait for the target to confirm the drop with `XdndFinished`.
const FINISH_TIMEOUT: Duration = Duration::from_secs(10);

/// Start dragging `files` (absolute paths) out of the window `own_window`.
/// Returns once the session is set up; the drag itself runs on a background thread
/// until the left mouse button is released.
pub fn start(own_window: u32, files: Vec<PathBuf>) -> Result<(), String> {
    let mut session = Session::new(own_window, &files).map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("xdnd-source".into())
        .spawn(move || match session.run() {
            Ok(outcome) => log::debug!("X11 drag finished: {outcome}"),
            Err(e) => log::error!("X11 drag failed: {e}"),
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

struct Atoms {
    aware: Atom,
    enter: Atom,
    position: Atom,
    status: Atom,
    leave: Atom,
    drop: Atom,
    finished: Atom,
    selection: Atom,
    type_list: Atom,
    action_list: Atom,
    action_copy: Atom,
    proxy: Atom,
    targets: Atom,
    uri_list: Atom,
    gnome_files: Atom,
}

impl Atoms {
    fn new(conn: &RustConnection) -> Res<Self> {
        let get = |name: &str| -> Res<Atom> {
            Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
        };
        Ok(Self {
            aware: get("XdndAware")?,
            enter: get("XdndEnter")?,
            position: get("XdndPosition")?,
            status: get("XdndStatus")?,
            leave: get("XdndLeave")?,
            drop: get("XdndDrop")?,
            finished: get("XdndFinished")?,
            selection: get("XdndSelection")?,
            type_list: get("XdndTypeList")?,
            action_list: get("XdndActionList")?,
            action_copy: get("XdndActionCopy")?,
            proxy: get("XdndProxy")?,
            targets: get("TARGETS")?,
            uri_list: get("text/uri-list")?,
            gnome_files: get("x-special/gnome-copied-files")?,
        })
    }
}

/// The window currently under the pointer that accepts XDND drops.
struct Target {
    /// The XDND-aware top-level window (what `XdndStatus` reports as its sender).
    toplevel: Window,
    /// Where messages are sent (`toplevel`, or its `XdndProxy`).
    dest: Window,
    version: u32,
    accepted: bool,
    waiting_for_status: bool,
    sent_at: Option<Instant>,
    sent_pos: (i16, i16),
}

struct Session {
    conn: RustConnection,
    root: Window,
    win: Window,
    own_window: Window,
    atoms: Atoms,
    uri_list: String,
    gnome_files: String,
    finished: bool,
    last_request: Option<Instant>,
}

impl Session {
    fn new(own_window: u32, files: &[PathBuf]) -> Res<Self> {
        if files.is_empty() {
            return Err("nothing to drag".into());
        }
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let atoms = Atoms::new(&conn)?;

        // An invisible window that owns the selection and receives XDND replies.
        let win = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new(),
        )?;
        // Some toolkits / XWayland read the offered types from the source window.
        conn.change_property32(
            PropMode::REPLACE,
            win,
            atoms.type_list,
            AtomEnum::ATOM,
            &[atoms.uri_list],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            win,
            atoms.action_list,
            AtomEnum::ATOM,
            &[atoms.action_copy],
        )?;
        conn.set_selection_owner(win, atoms.selection, CURRENT_TIME)?;
        conn.flush()?;
        if conn.get_selection_owner(atoms.selection)?.reply()?.owner != win {
            return Err("could not take ownership of XdndSelection".into());
        }

        let mut uris = Vec::new();
        for f in files {
            uris.push(path_to_uri(&std::path::absolute(f)?));
        }
        Ok(Self {
            conn,
            root,
            win,
            own_window,
            atoms,
            uri_list: uris.iter().map(|u| format!("{u}\r\n")).collect(),
            gnome_files: format!("copy\n{}", uris.join("\n")),
            finished: false,
            last_request: None,
        })
    }

    fn run(&mut self) -> Res<&'static str> {
        let mut target: Option<Target> = None;
        let mut outcome = "cancelled";
        let started = Instant::now();

        loop {
            self.drain_events(&mut target)?;
            let pointer = self.conn.query_pointer(self.root)?.reply()?;
            let button_down = u16::from(pointer.mask) & u16::from(KeyButMask::BUTTON1) != 0;
            let pos = (pointer.root_x, pointer.root_y);

            if !button_down {
                outcome = self.finish(&mut target)?;
                break;
            }
            if started.elapsed() > Duration::from_secs(600) {
                break;
            }

            // Which XDND-aware window is under the pointer now?
            let found = self.find_target()?;
            let changed = match (&target, &found) {
                (Some(t), Some(f)) => t.toplevel != f.toplevel,
                (None, None) => false,
                _ => true,
            };
            if changed {
                if let Some(old) = target.take() {
                    self.send(old.dest, self.atoms.leave, [self.win, 0, 0, 0, 0])?;
                }
                if let Some(new) = found {
                    self.send(
                        new.dest,
                        self.atoms.enter,
                        [self.win, new.version << 24, self.atoms.uri_list, 0, 0],
                    )?;
                    target = Some(new);
                }
            }
            if let Some(t) = target.as_mut() {
                // Don't flood the target: one outstanding position at a time (with a
                // timeout in case the target never answers).
                let stale = t
                    .sent_at
                    .is_some_and(|at| at.elapsed() > Duration::from_millis(250));
                let moved = t.sent_pos != pos || t.sent_at.is_none();
                if (moved && !t.waiting_for_status) || stale {
                    self.send_position(t, pos)?;
                }
            }
            self.conn.flush()?;
            std::thread::sleep(POLL);
        }

        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.flush();
        Ok(outcome)
    }

    /// The button was released: drop on the target if it accepted, otherwise leave.
    fn finish(&mut self, target: &mut Option<Target>) -> Res<&'static str> {
        if target.is_none() {
            return Ok("released over a window that doesn't accept drops");
        }
        // Give an in-flight status reply a moment to arrive.
        let deadline = Instant::now() + Duration::from_millis(250);
        while target.as_ref().is_some_and(|t| t.waiting_for_status) && Instant::now() < deadline {
            std::thread::sleep(POLL);
            self.drain_events(target)?;
        }
        let Some(t) = target.as_ref() else {
            return Ok("target went away");
        };
        let (dest, version) = (t.dest, t.version);
        if !t.accepted {
            self.send(dest, self.atoms.leave, [self.win, 0, 0, 0, 0])?;
            self.conn.flush()?;
            return Ok("target did not accept the drop");
        }
        self.send(dest, self.atoms.drop, [self.win, 0, CURRENT_TIME, 0, 0])?;
        self.conn.flush()?;

        // Serve the target's selection requests until it reports it is done.
        let deadline = Instant::now() + FINISH_TIMEOUT;
        while !self.finished && Instant::now() < deadline {
            self.drain_events(target)?;
            // XDND < 2 has no XdndFinished: settle shortly after the last request.
            if version < 2
                && self
                    .last_request
                    .is_some_and(|at| at.elapsed() > Duration::from_secs(1))
            {
                break;
            }
            std::thread::sleep(POLL);
        }
        Ok(if self.finished {
            "dropped"
        } else {
            "dropped (no confirmation from target)"
        })
    }

    fn send_position(&self, t: &mut Target, pos: (i16, i16)) -> Res<()> {
        let packed = ((pos.0 as u16 as u32) << 16) | (pos.1 as u16 as u32);
        self.send(
            t.dest,
            self.atoms.position,
            [self.win, 0, packed, CURRENT_TIME, self.atoms.action_copy],
        )?;
        t.waiting_for_status = true;
        t.sent_at = Some(Instant::now());
        t.sent_pos = pos;
        Ok(())
    }

    fn send(&self, window: Window, type_: Atom, data: [u32; 5]) -> Res<()> {
        let event = ClientMessageEvent {
            response_type: CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window,
            type_,
            data: data.into(),
        };
        self.conn
            .send_event(false, window, EventMask::NO_EVENT, event)?;
        Ok(())
    }

    fn drain_events(&mut self, target: &mut Option<Target>) -> Res<()> {
        while let Some(event) = self.conn.poll_for_event()? {
            match event {
                Event::ClientMessage(m) if m.type_ == self.atoms.status => {
                    let d = m.data.as_data32();
                    if let Some(t) = target.as_mut().filter(|t| t.toplevel == d[0]) {
                        t.accepted = d[1] & 1 != 0;
                        t.waiting_for_status = false;
                    }
                }
                Event::ClientMessage(m) if m.type_ == self.atoms.finished => self.finished = true,
                Event::SelectionRequest(r) => {
                    self.last_request = Some(Instant::now());
                    self.serve(&r)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Answer a request for the dragged data (`text/uri-list` plus a few aliases).
    fn serve(&self, r: &SelectionRequestEvent) -> Res<()> {
        let property = if r.property == NONE {
            r.target
        } else {
            r.property
        };
        let a = &self.atoms;
        let mut reply = property;
        if r.selection != a.selection {
            reply = NONE;
        } else if r.target == a.targets {
            self.conn.change_property32(
                PropMode::REPLACE,
                r.requestor,
                property,
                AtomEnum::ATOM,
                &[a.targets, a.uri_list, a.gnome_files],
            )?;
        } else if r.target == a.uri_list {
            self.conn.change_property8(
                PropMode::REPLACE,
                r.requestor,
                property,
                r.target,
                self.uri_list.as_bytes(),
            )?;
        } else if r.target == a.gnome_files {
            self.conn.change_property8(
                PropMode::REPLACE,
                r.requestor,
                property,
                r.target,
                self.gnome_files.as_bytes(),
            )?;
        } else {
            reply = NONE;
        }
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: r.time,
            requestor: r.requestor,
            selection: r.selection,
            target: r.target,
            property: reply,
        };
        self.conn
            .send_event(false, r.requestor, EventMask::NO_EVENT, notify)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Walk down the window tree under the pointer and return the innermost window
    /// that advertises `XdndAware`, ignoring our own window.
    fn find_target(&self) -> Res<Option<Target>> {
        let mut chain = Vec::new();
        let mut w = self.root;
        loop {
            let q = self.conn.query_pointer(w)?.reply()?;
            if q.child == NONE {
                break;
            }
            w = q.child;
            chain.push(w);
        }
        if chain.contains(&self.own_window) {
            return Ok(None);
        }
        for &w in chain.iter().rev() {
            let reply = self
                .conn
                .get_property(false, w, self.atoms.aware, AtomEnum::ATOM, 0, 1)?
                .reply()?;
            let Some(version) = reply.value32().and_then(|mut v| v.next()) else {
                continue;
            };
            if version < 3 {
                // Pre-v3 targets expect a different handshake; not worth supporting.
                continue;
            }
            return Ok(Some(Target {
                toplevel: w,
                dest: self.proxy_of(w)?.unwrap_or(w),
                version: version.min(VERSION),
                accepted: false,
                waiting_for_status: false,
                sent_at: None,
                sent_pos: (i16::MIN, i16::MIN),
            }));
        }
        Ok(None)
    }

    fn proxy_of(&self, w: Window) -> Res<Option<Window>> {
        let reply = self
            .conn
            .get_property(false, w, self.atoms.proxy, AtomEnum::WINDOW, 0, 1)?
            .reply()?;
        Ok(reply
            .value32()
            .and_then(|mut v| v.next())
            .filter(|&p| p != NONE))
    }
}

/// `file://` URI for an absolute path, percent-encoding everything but unreserved
/// characters and `/`.
fn path_to_uri(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn uris_are_percent_encoded() {
        assert_eq!(
            path_to_uri(Path::new("/tmp/a b/c#d.sii")),
            "file:///tmp/a%20b/c%23d.sii"
        );
        assert_eq!(path_to_uri(Path::new("/tmp/ä")), "file:///tmp/%C3%A4");
    }
}

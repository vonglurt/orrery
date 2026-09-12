//! SFTP version 3 -- the file half of the console.
//!
//! WHY VERSION 3 AND NOT A LATER ONE. OpenSSH's server speaks 3 and has since
//! before this fleet existed; versions 4 through 6 exist in the drafts and
//! almost nowhere else. Asking for a version no node will ever offer would be
//! a negotiation with one possible outcome and a second code path to maintain
//! for the outcome that never happens.
//!
//! IT IS A SUBSYSTEM, NOT A COMMAND. The console asks the node for `sftp` by
//! name and the node looks that name up in its own `Subsystem` line. `exec`
//! would have let the console name a binary and a path, which is the console
//! choosing what runs on somebody else's machine -- the same argument
//! `verbs.rs` makes about its allow-list, one layer down.
//!
//! THE TRANSPORT IS A TRAIT, AND THAT IS WHAT MAKES THIS TESTABLE. Over a
//! fleet node it is an `ssh::Session`; in the tests it is a pair of pipes with
//! OpenSSH's own `sftp-server` on the far end. There is no fake server in this
//! file and there should not be: a hand-written one would agree with this
//! file's reading of the draft, including wherever that reading is wrong, and
//! the real server is sitting on both machines this is developed on
//! (`/usr/libexec/sftp-server` on a Mac, `/usr/lib/ssh/sftp-server` on Alpine).
//!
//! PIPELINED, BECAUSE THE LATENCY IS THE TRANSFER. A 32 KB read that waits for
//! its answer before asking the next question moves one chunk per round trip;
//! on a museum LAN that is a few hundred round trips a second at best, and a
//! 200 MB image would take minutes it does not need to take. Eight requests
//! are kept in flight, answers are matched to offsets by request id, and the
//! file is written where each answer says rather than where it arrived.

use crate::ssh::{self, Buf, Cur};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

// The message numbers, from draft-ietf-secsh-filexfer-02.
mod fxp {
    pub const INIT: u8 = 1;
    pub const VERSION: u8 = 2;
    pub const OPEN: u8 = 3;
    pub const CLOSE: u8 = 4;
    pub const READ: u8 = 5;
    pub const WRITE: u8 = 6;
    pub const LSTAT: u8 = 7;
    pub const OPENDIR: u8 = 11;
    pub const READDIR: u8 = 12;
    pub const REMOVE: u8 = 13;
    pub const MKDIR: u8 = 14;
    pub const RMDIR: u8 = 15;
    pub const REALPATH: u8 = 16;
    pub const STAT: u8 = 17;
    pub const RENAME: u8 = 18;
    pub const STATUS: u8 = 101;
    pub const HANDLE: u8 = 102;
    pub const DATA: u8 = 103;
    pub const NAME: u8 = 104;
    pub const ATTRS: u8 = 105;
}

mod flag {
    pub const READ: u32 = 0x1;
    pub const WRITE: u32 = 0x2;
    pub const CREAT: u32 = 0x8;
    pub const TRUNC: u32 = 0x10;
}

/// The version this client speaks and the only one it accepts.
pub const VERSION: u32 = 3;

/// How much is asked for at once, and how many of those are outstanding.
///
/// 32 KB is what OpenSSH's own client uses and what a node's channel window is
/// sized for; eight in flight is a quarter of a megabyte of exposure, which is
/// enough to fill a hundred-megabit link at museum latencies and small enough
/// that a stalled transfer does not have megabytes of state to unwind.
const CHUNK: usize = 32 * 1024;
const DEPTH: usize = 8;

/// Anything two-way that bytes can be pushed through.
pub trait Stream {
    /// Whatever has arrived. Never blocks.
    fn read(&mut self) -> Result<Vec<u8>, String>;
    fn write(&mut self, data: &[u8]) -> Result<(), String>;
    /// True when the far end has finished and there is nothing left to read.
    fn ended(&mut self) -> bool;
}

impl Stream for ssh::Session {
    fn read(&mut self) -> Result<Vec<u8>, String> {
        ssh::Session::read(self)
    }
    fn write(&mut self, data: &[u8]) -> Result<(), String> {
        ssh::Session::write(self, data)
    }
    fn ended(&mut self) -> bool {
        self.finished().is_some()
    }
}

/// A child process on a pair of pipes -- `sftp-server` itself.
///
/// Not a `Pty`: a subsystem talks a binary protocol, and a terminal would
/// translate its bytes. `\n` becoming `\r\n` in the middle of a length field
/// is the kind of bug that takes an afternoon.
pub struct Pipe {
    child: std::process::Child,
    out: std::process::ChildStdout,
    input: std::process::ChildStdin,
    done: bool,
}

impl Pipe {
    pub fn spawn(argv: &[String]) -> Result<Pipe, String> {
        use std::os::fd::AsRawFd;
        use std::process::{Command, Stdio};
        if argv.is_empty() {
            return Err("nothing to run".into());
        }
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not run {}: {}", argv[0], e))?;
        let out = child.stdout.take().ok_or("no pipe from the server")?;
        let input = child.stdin.take().ok_or("no pipe to the server")?;
        crate::pty::set_nonblocking(out.as_raw_fd())?;
        Ok(Pipe {
            child,
            out,
            input,
            done: false,
        })
    }
}

impl Stream for Pipe {
    fn read(&mut self) -> Result<Vec<u8>, String> {
        let mut all = Vec::new();
        let mut buf = [0u8; 16384];
        loop {
            match self.out.read(&mut buf) {
                Ok(0) => {
                    self.done = true;
                    return Ok(all);
                }
                Ok(n) => {
                    all.extend_from_slice(&buf[..n]);
                    if n < buf.len() {
                        return Ok(all);
                    }
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => return Ok(all),
                Err(e) => return Err(format!("reading from sftp-server: {}", e)),
            }
        }
    }
    fn write(&mut self, data: &[u8]) -> Result<(), String> {
        self.input
            .write_all(data)
            .map_err(|e| format!("writing to sftp-server: {}", e))?;
        self.input.flush().map_err(|e| e.to_string())
    }
    fn ended(&mut self) -> bool {
        self.done
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One entry of a directory.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub mode: u32,
    pub mtime: u32,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.mode & 0o170000 == 0o040000
    }
    /// What the pane shows on the right of the row.
    pub fn measure(&self) -> String {
        if self.is_dir() {
            return "dir".into();
        }
        let n = self.size;
        if n < 1024 {
            format!("{} B", n)
        } else if n < 1024 * 1024 {
            format!("{} K", n / 1024)
        } else if n < 1024 * 1024 * 1024 {
            format!("{} M", n / (1024 * 1024))
        } else {
            format!("{:.1} G", n as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }
}

fn attrs(c: &mut Cur) -> Result<Entry, String> {
    let mut e = Entry::default();
    let flags = c.u32()?;
    if flags & 0x1 != 0 {
        e.size = c.u64()?;
    }
    if flags & 0x2 != 0 {
        let _uid = c.u32()?;
        let _gid = c.u32()?;
    }
    if flags & 0x4 != 0 {
        e.mode = c.u32()?;
    }
    if flags & 0x8 != 0 {
        let _atime = c.u32()?;
        e.mtime = c.u32()?;
    }
    if flags & 0x8000_0000 != 0 {
        let n = c.u32()?;
        for _ in 0..n {
            let _ = c.string()?;
            let _ = c.string()?;
        }
    }
    Ok(e)
}

/// The one status code that is not a failure.
const OK: u32 = 0;
const EOF: u32 = 1;

fn status_word(code: u32) -> &'static str {
    match code {
        0 => "no error",
        1 => "end of file",
        2 => "no such file",
        3 => "permission denied",
        4 => "the node refused",
        5 => "a message the node could not read",
        6 => "no connection",
        7 => "the connection was lost",
        8 => "the node does not support that",
        _ => "an unnamed failure",
    }
}

/// A conversation with one node's file system.
pub struct Sftp<T: Stream> {
    t: T,
    inbuf: Vec<u8>,
    next: u32,
    /// What the server said it speaks. Kept because a node answering 2 is a
    /// node this will refuse, and the number belongs in the refusal.
    pub version: u32,
}

impl Sftp<ssh::Session> {
    /// Open a connection to a node and ask for its `sftp` subsystem.
    pub fn open(d: &ssh::Dial) -> Result<Sftp<ssh::Session>, String> {
        let mut s = ssh::Session::open(d)?;
        s.subsystem("sftp")?;
        Sftp::start(s)
    }
}

impl Sftp<Pipe> {
    /// Run a server on pipes. This is how the tests reach OpenSSH's own.
    pub fn local(argv: &[String]) -> Result<Sftp<Pipe>, String> {
        Sftp::start(Pipe::spawn(argv)?)
    }

    /// Where `sftp-server` lives on the machines this runs on, or nothing.
    pub fn server_binary() -> Option<String> {
        for p in [
            "/usr/lib/ssh/sftp-server",
            "/usr/libexec/sftp-server",
            "/usr/lib/openssh/sftp-server",
            "/usr/libexec/openssh/sftp-server",
        ] {
            if Path::new(p).exists() {
                return Some(p.to_string());
            }
        }
        None
    }
}

impl<T: Stream> Sftp<T> {
    const TIMEOUT: Duration = Duration::from_secs(30);

    fn start(t: T) -> Result<Sftp<T>, String> {
        let mut s = Sftp {
            t,
            inbuf: Vec::new(),
            next: 1,
            version: 0,
        };
        // INIT carries a version where every other message carries a request
        // id, which is the one irregularity in the protocol and the one place
        // a generic "send a request" helper would be wrong.
        let mut b = Buf::new();
        b.u32(VERSION);
        s.raw(fxp::INIT, &b.take())?;
        let (kind, body) = s.packet()?;
        if kind != fxp::VERSION {
            return Err("the node did not answer the SFTP handshake".into());
        }
        let mut c = Cur::new(&body);
        s.version = c.u32()?;
        if s.version < VERSION {
            return Err(format!(
                "the node speaks SFTP {} and this console speaks {}",
                s.version, VERSION
            ));
        }
        Ok(s)
    }

    fn raw(&mut self, kind: u8, body: &[u8]) -> Result<(), String> {
        let mut out = Vec::with_capacity(body.len() + 5);
        out.extend_from_slice(&((body.len() + 1) as u32).to_be_bytes());
        out.push(kind);
        out.extend_from_slice(body);
        self.t.write(&out)
    }

    /// Send a request and return the id it will be answered with.
    fn request(&mut self, kind: u8, body: &[u8]) -> Result<u32, String> {
        let id = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        let mut full = Vec::with_capacity(body.len() + 4);
        full.extend_from_slice(&id.to_be_bytes());
        full.extend_from_slice(body);
        self.raw(kind, &full)?;
        Ok(id)
    }

    /// One packet, waiting for it.
    fn packet(&mut self) -> Result<(u8, Vec<u8>), String> {
        let start = Instant::now();
        loop {
            if self.inbuf.len() >= 4 {
                let len = u32::from_be_bytes([
                    self.inbuf[0],
                    self.inbuf[1],
                    self.inbuf[2],
                    self.inbuf[3],
                ]) as usize;
                // A length nobody could mean. The number is the server's and
                // arrives before anything has been parsed, so it is checked
                // rather than used to size an allocation.
                if len == 0 || len > 4 * 1024 * 1024 {
                    return Err(format!("an SFTP packet claiming to be {} bytes", len));
                }
                if self.inbuf.len() >= 4 + len {
                    let kind = self.inbuf[4];
                    let body = self.inbuf[5..4 + len].to_vec();
                    self.inbuf.drain(..4 + len);
                    return Ok((kind, body));
                }
            }
            let got = self.t.read()?;
            if got.is_empty() {
                if self.t.ended() {
                    return Err("the file transfer ended before the answer did".into());
                }
                if start.elapsed() > Self::TIMEOUT {
                    return Err("the node stopped answering".into());
                }
                std::thread::sleep(Duration::from_millis(2));
            } else {
                self.inbuf.extend_from_slice(&got);
            }
        }
    }

    /// The answer to one particular request. Answers to others are kept,
    /// because a pipelined transfer has several outstanding at once.
    fn answer(&mut self, id: u32) -> Result<(u8, Vec<u8>), String> {
        let mut held: Vec<(u8, Vec<u8>)> = Vec::new();
        loop {
            let (kind, body) = self.packet()?;
            if body.len() >= 4 {
                let got = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                if got == id {
                    // Anything held is put back in front of what has not
                    // arrived yet, in the order it came.
                    for (k, b) in held.into_iter().rev() {
                        let mut packet = Vec::new();
                        packet.extend_from_slice(&((b.len() + 1) as u32).to_be_bytes());
                        packet.push(k);
                        packet.extend_from_slice(&b);
                        self.inbuf.splice(0..0, packet);
                    }
                    return Ok((kind, body));
                }
            }
            held.push((kind, body));
            if held.len() > 64 {
                return Err("the node is answering questions nobody asked".into());
            }
        }
    }

    fn status(&mut self, id: u32) -> Result<u32, String> {
        let (kind, body) = self.answer(id)?;
        if kind != fxp::STATUS {
            return Err(format!("expected a status and got message {}", kind));
        }
        let mut c = Cur::new(&body);
        let _id = c.u32()?;
        Ok(c.u32()?)
    }

    fn ok(&mut self, id: u32, what: &str) -> Result<(), String> {
        let code = self.status(id)?;
        if code == OK {
            return Ok(());
        }
        Err(format!("{}: {}", what, status_word(code)))
    }

    fn handle(&mut self, id: u32, what: &str) -> Result<Vec<u8>, String> {
        let (kind, body) = self.answer(id)?;
        let mut c = Cur::new(&body);
        let _id = c.u32()?;
        match kind {
            fxp::HANDLE => Ok(c.string()?.to_vec()),
            fxp::STATUS => Err(format!("{}: {}", what, status_word(c.u32()?))),
            n => Err(format!("{}: message {} instead of a handle", what, n)),
        }
    }

    /// Resolve a path the way the node sees it. `.` is how the browser learns
    /// where it starts, because an SFTP session has no working directory of
    /// its own to ask about.
    pub fn realpath(&mut self, path: &str) -> Result<String, String> {
        let mut b = Buf::new();
        b.str(path);
        let id = self.request(fxp::REALPATH, &b.take())?;
        let (kind, body) = self.answer(id)?;
        let mut c = Cur::new(&body);
        let _id = c.u32()?;
        match kind {
            fxp::NAME => {
                let n = c.u32()?;
                if n < 1 {
                    return Err(format!("{}: the node named nothing", path));
                }
                Ok(c.text()?)
            }
            fxp::STATUS => Err(format!("{}: {}", path, status_word(c.u32()?))),
            n => Err(format!("{}: message {} instead of a name", path, n)),
        }
    }

    /// Everything in a directory, `.` and `..` left out.
    pub fn list(&mut self, dir: &str) -> Result<Vec<Entry>, String> {
        let mut b = Buf::new();
        b.str(dir);
        let id = self.request(fxp::OPENDIR, &b.take())?;
        let h = self.handle(id, dir)?;

        let mut out = Vec::new();
        loop {
            let mut b = Buf::new();
            b.string(&h);
            let id = self.request(fxp::READDIR, &b.take())?;
            let (kind, body) = self.answer(id)?;
            let mut c = Cur::new(&body);
            let _id = c.u32()?;
            match kind {
                fxp::NAME => {
                    let n = c.u32()?;
                    for _ in 0..n {
                        let name = c.text()?;
                        // The long name is `ls -l` output for a human and this
                        // console renders its own row, so it is read and
                        // dropped rather than skipped by arithmetic.
                        let _long = c.string()?;
                        let mut e = attrs(&mut c)?;
                        e.name = name;
                        if e.name != "." && e.name != ".." {
                            out.push(e);
                        }
                    }
                }
                fxp::STATUS => {
                    let code = c.u32()?;
                    if code == EOF {
                        break;
                    }
                    let _ = self.close(&h);
                    return Err(format!("{}: {}", dir, status_word(code)));
                }
                n => return Err(format!("{}: message {} in a listing", dir, n)),
            }
            // A directory with more entries than this is a directory nobody
            // is browsing; the guard is against a server that never says EOF.
            if out.len() > 20_000 {
                break;
            }
        }
        self.close(&h)?;
        // Directories first, then by name -- the order a person looks for a
        // file in, rather than the order the filesystem happens to hold them.
        out.sort_by(|a, b| {
            b.is_dir()
                .cmp(&a.is_dir())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    pub fn stat(&mut self, path: &str) -> Result<Entry, String> {
        self.stat_with(fxp::STAT, path)
    }

    /// `lstat` does not follow a symbolic link, which is what a listing wants
    /// and what a transfer does not.
    pub fn lstat(&mut self, path: &str) -> Result<Entry, String> {
        self.stat_with(fxp::LSTAT, path)
    }

    fn stat_with(&mut self, kind: u8, path: &str) -> Result<Entry, String> {
        let mut b = Buf::new();
        b.str(path);
        let id = self.request(kind, &b.take())?;
        let (kind, body) = self.answer(id)?;
        let mut c = Cur::new(&body);
        let _id = c.u32()?;
        match kind {
            fxp::ATTRS => {
                let mut e = attrs(&mut c)?;
                e.name = path.rsplit('/').next().unwrap_or(path).to_string();
                Ok(e)
            }
            fxp::STATUS => Err(format!("{}: {}", path, status_word(c.u32()?))),
            n => Err(format!("{}: message {} instead of attributes", path, n)),
        }
    }

    fn close(&mut self, handle: &[u8]) -> Result<(), String> {
        let mut b = Buf::new();
        b.string(handle);
        let id = self.request(fxp::CLOSE, &b.take())?;
        self.ok(id, "closing a file")
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), String> {
        let mut b = Buf::new();
        b.str(path).u32(0);
        let id = self.request(fxp::MKDIR, &b.take())?;
        self.ok(id, path)
    }

    pub fn rmdir(&mut self, path: &str) -> Result<(), String> {
        let mut b = Buf::new();
        b.str(path);
        let id = self.request(fxp::RMDIR, &b.take())?;
        self.ok(id, path)
    }

    pub fn remove(&mut self, path: &str) -> Result<(), String> {
        let mut b = Buf::new();
        b.str(path);
        let id = self.request(fxp::REMOVE, &b.take())?;
        self.ok(id, path)
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let mut b = Buf::new();
        b.str(from).str(to);
        let id = self.request(fxp::RENAME, &b.take())?;
        self.ok(id, from)
    }
}

// ---------------------------------------------------------------------------
// Transfers
// ---------------------------------------------------------------------------

impl<T: Stream> Sftp<T> {
    /// The next answer to arrive, whoever asked for it.
    ///
    /// `answer` waits for one particular id and holds the rest; a pipelined
    /// transfer wants the opposite -- take whatever came, look up which chunk
    /// it belongs to, and put it where that chunk goes. WHICH IS WHY THE FILE
    /// IS WRITTEN BY SEEKING rather than by appending: the answers are allowed
    /// to arrive in any order and, on a busy node, sometimes do.
    fn any_answer(&mut self) -> Result<(u8, u32, Vec<u8>), String> {
        let (kind, body) = self.packet()?;
        if body.len() < 4 {
            return Err("an SFTP answer with no request id".into());
        }
        let id = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
        Ok((kind, id, body))
    }

    /// Copy a file off a node. Returns how many bytes arrived.
    ///
    /// `progress` is called as chunks land, so a pane can draw a bar without
    /// this function knowing what a pane is.
    pub fn get(
        &mut self,
        remote: &str,
        local: &Path,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<u64, String> {
        let meta = self.stat(remote)?;
        if meta.is_dir() {
            return Err(format!("{} is a directory", remote));
        }
        let total = meta.size;

        let mut b = Buf::new();
        b.str(remote).u32(flag::READ).u32(0);
        let id = self.request(fxp::OPEN, &b.take())?;
        let h = self.handle(id, remote)?;

        let mut f = std::fs::File::create(local)
            .map_err(|e| format!("{}: {}", local.display(), e))?;

        // Every chunk that still has to be asked for, and every one that has
        // been asked for and not yet answered.
        let mut want: Vec<(u64, usize)> = Vec::new();
        let mut at = 0u64;
        while at < total {
            let n = std::cmp::min(CHUNK as u64, total - at) as usize;
            want.push((at, n));
            at += n as u64;
        }
        want.reverse(); // popped from the end, so issued in order
        let mut inflight: HashMap<u32, (u64, usize)> = HashMap::new();
        let mut done = 0u64;

        let result = (|| -> Result<u64, String> {
            loop {
                while inflight.len() < DEPTH {
                    let Some((off, len)) = want.pop() else { break };
                    let mut b = Buf::new();
                    b.string(&h).u64(off).u32(len as u32);
                    let id = self.request(fxp::READ, &b.take())?;
                    inflight.insert(id, (off, len));
                }
                if inflight.is_empty() {
                    return Ok(done);
                }
                let (kind, id, body) = self.any_answer()?;
                let Some((off, len)) = inflight.remove(&id) else {
                    continue;
                };
                let mut c = Cur::new(&body);
                let _id = c.u32()?;
                match kind {
                    fxp::DATA => {
                        let data = c.string()?;
                        f.seek(SeekFrom::Start(off))
                            .map_err(|e| format!("{}: {}", local.display(), e))?;
                        f.write_all(data)
                            .map_err(|e| format!("{}: {}", local.display(), e))?;
                        done += data.len() as u64;
                        progress(done, total);
                        // A SHORT READ IS LEGAL AND IS NOT AN END. The server
                        // may answer with less than was asked for; the rest of
                        // that chunk goes back on the queue rather than being
                        // silently lost, which is how a transfer ends up one
                        // block short and corrupt.
                        if data.len() < len {
                            want.push((off + data.len() as u64, len - data.len()));
                        }
                    }
                    fxp::STATUS => {
                        let code = c.u32()?;
                        if code == EOF {
                            // The file is shorter than its own stat said.
                            // Everything still queued is past the end.
                            want.clear();
                            continue;
                        }
                        return Err(format!("{}: {}", remote, status_word(code)));
                    }
                    n => return Err(format!("{}: message {} during a read", remote, n)),
                }
            }
        })();

        let closed = self.close(&h);
        let got = result?;
        closed?;
        f.flush().map_err(|e| e.to_string())?;
        Ok(got)
    }

    /// Copy a file onto a node. Returns how many bytes were sent.
    pub fn put(
        &mut self,
        local: &Path,
        remote: &str,
        mode: u32,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<u64, String> {
        let meta = std::fs::metadata(local).map_err(|e| format!("{}: {}", local.display(), e))?;
        if meta.is_dir() {
            return Err(format!("{} is a directory", local.display()));
        }
        let total = meta.len();
        let mut f = std::fs::File::open(local).map_err(|e| format!("{}: {}", local.display(), e))?;

        let mut b = Buf::new();
        // CREAT and TRUNC together: a transfer that landed on an existing
        // longer file and did not truncate would leave the tail of the old one
        // behind the new, which reads as a corrupt image rather than as an
        // error.
        b.str(remote)
            .u32(flag::WRITE | flag::CREAT | flag::TRUNC)
            .u32(0x4)
            .u32(mode);
        let id = self.request(fxp::OPEN, &b.take())?;
        let h = self.handle(id, remote)?;

        let mut inflight: HashMap<u32, usize> = HashMap::new();
        let mut at = 0u64;
        let mut done = 0u64;
        let mut eof = false;

        let result = (|| -> Result<u64, String> {
            loop {
                while inflight.len() < DEPTH && !eof {
                    let mut buf = vec![0u8; CHUNK];
                    let mut n = 0;
                    while n < CHUNK {
                        match f.read(&mut buf[n..]) {
                            Ok(0) => break,
                            Ok(k) => n += k,
                            Err(e) => return Err(format!("{}: {}", local.display(), e)),
                        }
                    }
                    if n == 0 {
                        eof = true;
                        break;
                    }
                    buf.truncate(n);
                    let mut b = Buf::new();
                    b.string(&h).u64(at).string(&buf);
                    let id = self.request(fxp::WRITE, &b.take())?;
                    inflight.insert(id, n);
                    at += n as u64;
                    if n < CHUNK {
                        eof = true;
                    }
                }
                if inflight.is_empty() {
                    return Ok(done);
                }
                let (kind, id, body) = self.any_answer()?;
                let Some(n) = inflight.remove(&id) else {
                    continue;
                };
                if kind != fxp::STATUS {
                    return Err(format!("{}: message {} during a write", remote, kind));
                }
                let mut c = Cur::new(&body);
                let _id = c.u32()?;
                let code = c.u32()?;
                if code != OK {
                    return Err(format!("{}: {}", remote, status_word(code)));
                }
                done += n as u64;
                progress(done, total);
            }
        })();

        let closed = self.close(&h);
        let sent = result?;
        closed?;
        Ok(sent)
    }
}

/// Send one local file to many nodes, and say what happened to each.
///
/// THE FAN-OUT IS SEQUENTIAL, WHICH IS A CHOICE. Eight simultaneous transfers
/// share one hundred-megabit switch and arrive at the same time as eight
/// sequential ones, except that a failure in the middle of the parallel
/// version leaves seven half-written files and no clear account of which.
/// `verb_each` in `fleet.rs` made the same call for the same reason.
pub fn send_each(
    dials: &[(String, ssh::Dial)],
    local: &Path,
    remote_dir: &str,
    mut progress: impl FnMut(&str, u64, u64),
) -> Vec<(String, Result<u64, String>)> {
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let mut out = Vec::new();
    for (node, d) in dials {
        let target = format!("{}/{}", remote_dir.trim_end_matches('/'), name);
        let r = Sftp::open(d).and_then(|mut s| {
            s.put(local, &target, 0o644, |a, b| progress(node, a, b))
        });
        out.push((node.clone(), r));
    }
    out
}

// ---------------------------------------------------------------------------
// Proved against OpenSSH's own server
// ---------------------------------------------------------------------------
//
// NO FAKE SERVER, ON PURPOSE. `rfb.rs` and `ssh.rs` both carry one because
// there was no way to run the real thing in a unit test; here there is. The
// binary is sitting on both machines this is developed on, it speaks the
// protocol a node speaks because it IS the protocol a node speaks, and a fake
// would only ever agree with this file's reading of the draft.
//
// The tests step aside when the binary is missing, the same arrangement the
// Wayland and sshd tests have -- and unlike those, it is almost never missing.

#[cfg(test)]
mod tests {
    use super::*;

    /// A server on pipes, and a directory to be destructive in.
    fn server() -> Option<(Sftp<Pipe>, std::path::PathBuf)> {
        let bin = Sftp::server_binary()?;
        let dir = std::env::temp_dir().join(format!(
            "orrery-sftp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Sftp::local(&[bin]).expect("sftp-server should start");
        Some((s, dir))
    }

    fn p(dir: &Path, name: &str) -> String {
        dir.join(name).to_string_lossy().to_string()
    }

    #[test]
    fn the_handshake_settles_on_version_three() {
        let Some((s, dir)) = server() else { return };
        assert!(s.version >= VERSION, "the server offered {}", s.version);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_directory_lists_with_sizes_and_kinds_and_no_dot_entries() {
        let Some((mut s, dir)) = server() else { return };
        std::fs::write(dir.join("answers.txt"), b"COPAL_FLEET='museum'\n").unwrap();
        std::fs::write(dir.join("image.img"), vec![7u8; 4096]).unwrap();
        std::fs::create_dir(dir.join("cards")).unwrap();

        let list = s.list(&p(&dir, "")).unwrap();
        let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["cards", "answers.txt", "image.img"], "{:?}", names);
        assert!(list[0].is_dir(), "a directory did not read as one");
        assert!(!list[1].is_dir());
        assert_eq!(list[2].size, 4096);
        assert_eq!(list[2].measure(), "4 K");
        // `.` and `..` are the server's business and never the browser's.
        assert!(!names.contains(&"."), "the listing carried a dot entry");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_comes_back_byte_for_byte_across_many_chunks() {
        // Larger than one chunk and not a multiple of one, so the pipelining,
        // the final short chunk and the out-of-order write all happen.
        let Some((mut s, dir)) = server() else { return };
        let n = CHUNK * 5 + 1234;
        let body: Vec<u8> = (0..n).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::write(dir.join("big.img"), &body).unwrap();

        let out = dir.join("copy.img");
        let mut last = (0u64, 0u64);
        let got = s
            .get(&p(&dir, "big.img"), &out, |a, b| last = (a, b))
            .unwrap();
        assert_eq!(got, n as u64);
        assert_eq!(last, (n as u64, n as u64), "progress did not finish");
        assert_eq!(std::fs::read(&out).unwrap(), body, "the copy differs");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_goes_up_byte_for_byte_and_replaces_what_was_there() {
        let Some((mut s, dir)) = server() else { return };
        let n = CHUNK * 3 + 7;
        let body: Vec<u8> = (0..n).map(|i| (i % 256) as u8).collect();
        let src = dir.join("source.bin");
        std::fs::write(&src, &body).unwrap();

        // Something longer already at the target: without TRUNC its tail would
        // survive behind the new file and the result would be silently wrong.
        std::fs::write(dir.join("dest.bin"), vec![0xaau8; n * 2]).unwrap();

        let sent = s.put(&src, &p(&dir, "dest.bin"), 0o644, |_, _| {}).unwrap();
        assert_eq!(sent, n as u64);
        assert_eq!(std::fs::read(dir.join("dest.bin")).unwrap(), body);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_empty_file_is_a_transfer_and_not_a_hang() {
        let Some((mut s, dir)) = server() else { return };
        std::fs::write(dir.join("empty"), b"").unwrap();
        let out = dir.join("empty-copy");
        assert_eq!(s.get(&p(&dir, "empty"), &out, |_, _| {}).unwrap(), 0);
        assert_eq!(std::fs::read(&out).unwrap().len(), 0);

        let up = s.put(&out, &p(&dir, "empty-up"), 0o644, |_, _| {}).unwrap();
        assert_eq!(up, 0);
        assert!(dir.join("empty-up").exists(), "the empty file was not created");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_mode_asked_for_is_the_mode_that_lands() {
        let Some((mut s, dir)) = server() else { return };
        let src = dir.join("script.sh");
        std::fs::write(&src, b"#!/bin/sh\necho hello\n").unwrap();
        s.put(&src, &p(&dir, "landed.sh"), 0o755, |_, _| {}).unwrap();
        let e = s.stat(&p(&dir, "landed.sh")).unwrap();
        assert_eq!(e.mode & 0o777, 0o755, "mode {:o}", e.mode);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn making_moving_and_removing_things_all_work_and_all_report() {
        let Some((mut s, dir)) = server() else { return };
        s.mkdir(&p(&dir, "stage")).unwrap();
        assert!(dir.join("stage").is_dir());

        std::fs::write(dir.join("stage/one"), b"x").unwrap();
        s.rename(&p(&dir, "stage/one"), &p(&dir, "stage/two")).unwrap();
        assert!(dir.join("stage/two").exists());

        s.remove(&p(&dir, "stage/two")).unwrap();
        assert!(!dir.join("stage/two").exists());
        s.rmdir(&p(&dir, "stage")).unwrap();
        assert!(!dir.join("stage").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn every_way_a_path_can_be_wrong_is_a_sentence() {
        let Some((mut s, dir)) = server() else { return };
        // Each of these is something an operator will do, and none of them
        // should arrive as a number or a panic.
        let e = s.list(&p(&dir, "nowhere")).unwrap_err();
        assert!(e.contains("no such file"), "{}", e);
        let e = s.stat(&p(&dir, "nowhere")).unwrap_err();
        assert!(e.contains("no such file"), "{}", e);
        let e = s
            .get(&p(&dir, "nowhere"), &dir.join("x"), |_, _| {})
            .unwrap_err();
        assert!(e.contains("no such file"), "{}", e);
        let e = s.rmdir(&p(&dir, "nowhere")).unwrap_err();
        assert!(!e.is_empty());

        // A directory is not a file, in either direction.
        std::fs::create_dir(dir.join("adir")).unwrap();
        let e = s
            .get(&p(&dir, "adir"), &dir.join("x"), |_, _| {})
            .unwrap_err();
        assert!(e.contains("is a directory"), "{}", e);
        let e = s
            .put(&dir, &p(&dir, "x"), 0o644, |_, _| {})
            .unwrap_err();
        assert!(e.contains("is a directory"), "{}", e);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_node_is_asked_where_it_thinks_it_is() {
        let Some((mut s, dir)) = server() else { return };
        // `.` is how the browser finds its starting point, because an SFTP
        // session has no working directory to ask about.
        let here = s.realpath(".").unwrap();
        assert!(here.starts_with('/'), "realpath gave {:?}", here);
        // And a path with a `..` in it comes back resolved, which is what
        // stops the browser accumulating one every time somebody goes up.
        let messy = format!("{}/./", dir.to_string_lossy());
        let clean = s.realpath(&messy).unwrap();
        assert!(!clean.contains("/./"), "realpath left {:?}", clean);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_size_reads_the_way_a_person_says_it() {
        let k = |n: u64, mode: u32| Entry { name: "x".into(), size: n, mode, mtime: 0 }.measure();
        assert_eq!(k(512, 0o100644), "512 B");
        assert_eq!(k(2048, 0o100644), "2 K");
        assert_eq!(k(5 * 1024 * 1024, 0o100644), "5 M");
        assert_eq!(k(3 * 1024 * 1024 * 1024, 0o100644), "3.0 G");
        assert_eq!(k(4096, 0o040755), "dir", "a directory has no size worth saying");
    }
}

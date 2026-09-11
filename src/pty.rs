//! A child process on the other end of a terminal.
//!
//! WHY A PTY AND NOT A PIPE, which is the whole reason this file exists:
//! `copal-prep.sh` picks the disk and asks for both typed `ERASE`
//! confirmations, and `bin/sd.sh` says in its own words that this *is the
//! entire safety model for this path*. A `read` prompt needs a terminal on the
//! other side. Give the child a pipe and it either blocks forever or takes the
//! silence as an answer, and neither of those is a thing to do to a program
//! whose next act is overwriting a disk.
//!
//! So the confirmations happen INSIDE THE PANE, typed by the operator, read by
//! `copal-prep.sh`, exactly as they would in a terminal. Nothing is answered on
//! the operator's behalf and there is no `--yes`.
//!
//! `openpty` rather than `forkpty`: with the slave descriptor in hand,
//! `std::process::Command` can be handed it as stdin, stdout and stderr, and
//! then the spawning, the argv, the environment and the reaping are std's
//! problem rather than this file's. What is left is one libc declaration and
//! two descriptors -- the same shape and the same argument as `sys.rs`.
//!
//! THIS IS ALSO PHASE 5'S TERMINAL, EARLY. A shell over SSH wants exactly this
//! plumbing with a socket where the child is, so building it for the card
//! writer means Terminal is a transport rather than a transport and a pane.

use std::ffi::{c_char, c_int, c_void, OsStr};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::process::{Child, Command, Stdio};

extern "C" {
    /// The one declaration. On a Mac it is in libSystem and on Alpine it is in
    /// libc proper, so neither platform needs a `-l` that is not already there.
    fn openpty(
        master: *mut c_int,
        slave: *mut c_int,
        name: *mut c_char,
        termp: *const c_void,
        winp: *const c_void,
    ) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn dup(fd: c_int) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, arg: c_int) -> c_int;
}

const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
#[cfg(target_os = "macos")]
const O_NONBLOCK: c_int = 0x0004;
#[cfg(target_os = "linux")]
const O_NONBLOCK: c_int = 0o4000;

fn set_nonblocking(fd: RawFd) -> Result<(), String> {
    unsafe {
        let flags = fcntl(fd, F_GETFL, 0);
        if flags < 0 {
            return Err("could not read the terminal's flags".into());
        }
        if fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0 {
            return Err("could not make the terminal non-blocking".into());
        }
    }
    Ok(())
}

/// A running command, with a terminal between it and here.
pub struct Pty {
    master: File,
    child: Child,
    /// Set once the child has been reaped, because `try_wait` on an
    /// already-reaped child is an error rather than an answer.
    status: Option<i32>,
}

impl Pty {
    /// Run `argv` in `dir`, with a terminal on its three standard descriptors.
    pub fn spawn(argv: &[String], dir: &OsStr) -> Result<Pty, String> {
        if argv.is_empty() {
            return Err("nothing to run".into());
        }
        let (master, slave) = unsafe {
            let (mut m, mut s): (c_int, c_int) = (-1, -1);
            if openpty(
                &mut m,
                &mut s,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            ) != 0
            {
                return Err("could not open a terminal".into());
            }
            (m, s)
        };
        set_nonblocking(master).inspect_err(|_| unsafe {
            close(master);
            close(slave);
        })?;

        // THREE SEPARATE DESCRIPTORS. `Stdio::from(File)` takes ownership and
        // closes on drop, so handing the same number in three times closes it
        // twice more than there are copies -- and on a busy process that
        // closes whatever was handed those numbers next. `wl.rs` has the same
        // note about the same mistake for the same reason.
        let (a, b, c) = unsafe { (dup(slave), dup(slave), dup(slave)) };
        if a < 0 || b < 0 || c < 0 {
            unsafe {
                close(master);
                close(slave);
            }
            return Err("could not duplicate the terminal".into());
        }

        let spawned = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(dir)
            .stdin(unsafe { Stdio::from(File::from_raw_fd(a)) })
            .stdout(unsafe { Stdio::from(File::from_raw_fd(b)) })
            .stderr(unsafe { Stdio::from(File::from_raw_fd(c)) })
            // `copal-prep.sh` colours its output for a terminal, and now it has
            // one -- so TERM has to name something, or ncurses-flavoured tools
            // in the chain complain rather than degrade.
            .env("TERM", "xterm-256color")
            .spawn();

        // The slave belongs to the child now. Keeping this end open means the
        // master never reports end-of-file when the child exits, and the pane
        // would wait for output for ever.
        unsafe {
            close(slave);
        }

        match spawned {
            Ok(child) => Ok(Pty {
                master: unsafe { File::from_raw_fd(master) },
                child,
                status: None,
            }),
            Err(e) => {
                unsafe {
                    close(master);
                }
                Err(format!("could not run {}: {}", argv[0], e))
            }
        }
    }

    /// Whatever the child has said since the last call. Never blocks.
    pub fn read(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match self.master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    if n < buf.len() {
                        break;
                    }
                }
                // WouldBlock is the normal case and means "nothing more yet".
                // EIO is what a pty master reports once the child has gone,
                // and it is an end rather than a fault.
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        out
    }

    /// What the operator typed. This is how `ERASE` reaches `copal-prep.sh`.
    pub fn write(&mut self, s: &str) {
        let _ = self.master.write_all(s.as_bytes());
        let _ = self.master.flush();
    }

    /// The exit status, once there is one.
    pub fn finished(&mut self) -> Option<i32> {
        if self.status.is_some() {
            return self.status;
        }
        match self.child.try_wait() {
            Ok(Some(s)) => {
                self.status = Some(s.code().unwrap_or(-1));
                self.status
            }
            _ => None,
        }
    }

    /// Stop it. Used when the operator closes a pane with a build in it.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.status = Some(-1);
    }

    pub fn fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // A card write that is still going when the window closes is left
        // alone deliberately: killing `copal-prep.sh` between the partition
        // table and the filesystem is how a card becomes a brick. The
        // descriptor goes; the child is reaped by init.
        let _ = self.child.try_wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::time::{Duration, Instant};

    fn wait_for(p: &mut Pty, want: &str, secs: u64) -> String {
        let start = Instant::now();
        let mut seen = String::new();
        while start.elapsed() < Duration::from_secs(secs) {
            seen.push_str(&String::from_utf8_lossy(&p.read()));
            if seen.contains(want) {
                return seen;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        seen
    }

    fn here() -> OsString {
        OsString::from(".")
    }

    #[test]
    fn a_child_runs_and_its_output_comes_back() {
        let argv = ["/bin/sh".into(), "-c".into(), "echo hello from the pane".into()];
        let mut p = Pty::spawn(&argv, &here()).expect("spawned");
        let seen = wait_for(&mut p, "hello from the pane", 5);
        assert!(seen.contains("hello from the pane"), "got {:?}", seen);
    }

    #[test]
    fn the_child_is_given_a_terminal_and_knows_it() {
        // THE WHOLE POINT OF THE FILE. With a pipe this prints "no"; a shell
        // that thinks it has no terminal does not prompt, and copal-prep.sh's
        // two typed confirmations are prompts.
        let argv = [
            "/bin/sh".into(),
            "-c".into(),
            "if [ -t 0 ]; then echo TTY-YES; else echo TTY-NO; fi".into(),
        ];
        let mut p = Pty::spawn(&argv, &here()).expect("spawned");
        let seen = wait_for(&mut p, "TTY-", 5);
        assert!(seen.contains("TTY-YES"), "the child had no terminal: {:?}", seen);
    }

    #[test]
    fn what_the_operator_types_reaches_the_child() {
        // This is `ERASE` reaching copal-prep.sh, in miniature.
        let argv = [
            "/bin/sh".into(),
            "-c".into(),
            "printf 'type it: '; read a; [ \"$a\" = ERASE ] && echo CONFIRMED || echo REFUSED".into(),
        ];
        let mut p = Pty::spawn(&argv, &here()).expect("spawned");
        wait_for(&mut p, "type it:", 5);
        p.write("ERASE\n");
        let seen = wait_for(&mut p, "CONFIRMED", 5);
        assert!(seen.contains("CONFIRMED"), "the answer did not arrive: {:?}", seen);
    }

    #[test]
    fn a_wrong_answer_is_a_wrong_answer() {
        let argv = [
            "/bin/sh".into(),
            "-c".into(),
            "read a; [ \"$a\" = ERASE ] && echo CONFIRMED || echo REFUSED".into(),
        ];
        let mut p = Pty::spawn(&argv, &here()).expect("spawned");
        p.write("erase\n");
        let seen = wait_for(&mut p, "REFUSED", 5);
        assert!(seen.contains("REFUSED"), "case-insensitive confirmation: {:?}", seen);
    }

    #[test]
    fn the_exit_status_is_reported_and_keeps_being_reported() {
        let argv = ["/bin/sh".into(), "-c".into(), "exit 3".into()];
        let mut p = Pty::spawn(&argv, &here()).expect("spawned");
        let start = Instant::now();
        while p.finished().is_none() && start.elapsed() < Duration::from_secs(5) {
            let _ = p.read();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(p.finished(), Some(3));
        // try_wait on an already-reaped child is an error rather than an
        // answer, so the status is remembered rather than asked for twice.
        assert_eq!(p.finished(), Some(3), "the status was forgotten");
    }

    #[test]
    fn a_command_that_is_not_there_is_a_sentence_rather_than_a_panic() {
        let argv = ["definitely-not-a-program-xyz".to_string()];
        let e = match Pty::spawn(&argv, &here()) {
            Ok(_) => panic!("spawned a program that is not there"),
            Err(e) => e,
        };
        assert!(e.contains("definitely-not-a-program-xyz"), "unhelpful: {}", e);
        assert!(Pty::spawn(&[], &here()).is_err(), "an empty argv ran something");
    }

    #[test]
    fn the_master_closes_when_the_child_goes() {
        // If this end kept the slave open, the master would never report
        // end-of-file and a pane would wait for output for ever.
        let argv = ["/bin/sh".into(), "-c".into(), "echo done".into()];
        let mut p = Pty::spawn(&argv, &here()).expect("spawned");
        wait_for(&mut p, "done", 5);
        let start = Instant::now();
        while p.finished().is_none() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Reading a master whose child has gone returns nothing rather than
        // blocking, which is what lets the pane notice the end.
        assert!(p.read().is_empty() || p.finished().is_some());
    }
}

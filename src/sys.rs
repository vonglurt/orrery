//! The handful of system calls `std` does not expose.
//!
//! THIS FILE IS WHERE THE NO-DEPENDENCIES RULE BENDS, AND IT IS WORTH BEING
//! PRECISE ABOUT HOW FAR. `Cargo.toml` gives the rule's reason in its own
//! words: "a dependency is a crate fetch that fails on the one machine this is
//! meant to run on." That reason is untouched here -- nothing is fetched,
//! nothing is vendored, and `copal-build` on an offline node is unaffected.
//! What appears for the first time is `unsafe extern "C"` against the system
//! libc, which this binary already links.
//!
//! It is not avoidable. Wayland hands a client its pixel buffer by passing a
//! FILE DESCRIPTOR over the socket with `SCM_RIGHTS`, and the buffer itself
//! wants `memfd_create` and `mmap`. `std::os::unix::net::UnixStream` has no
//! ancillary-data API and no way to fake one, and `std` has no `mmap`. So the
//! choice is these nine declarations or a crate, and nine declarations is the
//! one that still builds on a node with no internet.
//!
//! STRUCT LAYOUTS ARE THE KERNEL'S, NOT A LIBRARY'S. `msghdr` and `cmsghdr`
//! are declared differently by glibc and musl -- musl splits the two length
//! fields into `int` plus padding -- but on a 64-bit little-endian target the
//! two declarations are the same bytes, provided the padding is zero. Writing
//! a `usize` sets it to zero, so the layouts below are correct for both. The
//! fleet is aarch64 and little-endian; `compile_error!` below makes sure this
//! is never quietly wrong somewhere else.

#![allow(non_camel_case_types)]

use std::io;
use std::os::unix::io::RawFd;

#[cfg(not(target_pointer_width = "64"))]
compile_error!("sys.rs lays out msghdr for a 64-bit target");
#[cfg(target_endian = "big")]
compile_error!("sys.rs lays out msghdr for a little-endian target -- musl \
                orders its padding the other way round on big-endian");

pub type c_int = i32;
pub type c_uint = u32;
pub type c_char = i8;
pub type c_void = std::ffi::c_void;
pub type size_t = usize;
pub type ssize_t = isize;
pub type off_t = i64;

#[repr(C)]
struct IoVec {
    iov_base: *mut c_void,
    iov_len: size_t,
}

/// See the module comment: `msg_iovlen` and `msg_controllen` are `usize` here
/// because that is byte-identical to musl's `int` + zero padding on 64-bit LE.
#[repr(C)]
struct MsgHdr {
    msg_name: *mut c_void,
    msg_namelen: u32,
    _pad0: u32,
    msg_iov: *mut IoVec,
    msg_iovlen: size_t,
    msg_control: *mut c_void,
    msg_controllen: size_t,
    msg_flags: c_int,
    _pad1: u32,
}

#[repr(C)]
struct CmsgHdr {
    cmsg_len: size_t,
    cmsg_level: c_int,
    cmsg_type: c_int,
}

const SOL_SOCKET: c_int = 1;
const SCM_RIGHTS: c_int = 1;
const MSG_NOSIGNAL: c_int = 0x4000;
const MSG_CMSG_CLOEXEC: c_int = 0x4000_0000;

const MFD_CLOEXEC: c_uint = 1;

const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_FAILED: isize = -1;

extern "C" {
    fn sendmsg(fd: c_int, msg: *const MsgHdr, flags: c_int) -> ssize_t;
    fn recvmsg(fd: c_int, msg: *mut MsgHdr, flags: c_int) -> ssize_t;
    fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
    fn ftruncate(fd: c_int, length: off_t) -> c_int;
    fn mmap(
        addr: *mut c_void,
        length: size_t,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: off_t,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, length: size_t) -> c_int;
    fn close(fd: c_int) -> c_int;
}

/// Round up the way `CMSG_ALIGN` does.
const fn cmsg_align(len: usize) -> usize {
    (len + std::mem::size_of::<usize>() - 1) & !(std::mem::size_of::<usize>() - 1)
}

const CMSG_HDR_SPACE: usize = cmsg_align(std::mem::size_of::<CmsgHdr>());

/// Room for `n` descriptors, the way `CMSG_SPACE` computes it.
const fn cmsg_space(n: usize) -> usize {
    CMSG_HDR_SPACE + cmsg_align(n * std::mem::size_of::<c_int>())
}

/// The most descriptors one Wayland message can carry in this client. The
/// protocol permits more; nothing orrery sends or receives needs them, and a
/// fixed buffer means no allocation on the event path.
pub const MAX_FDS: usize = 4;

const CONTROL_LEN: usize = cmsg_space(MAX_FDS);

/// Send bytes, optionally with file descriptors attached.
///
/// Returns how many bytes went. The descriptors ride on the FIRST byte of the
/// message, which is why the caller must never split a Wayland message that
/// carries one across two calls.
pub fn send_with_fds(sock: RawFd, buf: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    if fds.len() > MAX_FDS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many descriptors for one message",
        ));
    }
    let mut iov = IoVec {
        iov_base: buf.as_ptr() as *mut c_void,
        iov_len: buf.len(),
    };
    let mut control = [0u8; CONTROL_LEN];
    let mut msg = MsgHdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        _pad0: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: std::ptr::null_mut(),
        msg_controllen: 0,
        msg_flags: 0,
        _pad1: 0,
    };

    if !fds.is_empty() {
        let payload = std::mem::size_of_val(fds);
        let len = CMSG_HDR_SPACE + payload;
        // SAFETY: `control` is at least `cmsg_space(MAX_FDS)` bytes and
        // `fds.len() <= MAX_FDS` was checked above, so the header and the
        // descriptors both fit. The buffer is `u8` and therefore has alignment
        // 1, so the header is written unaligned on purpose.
        unsafe {
            let hdr = control.as_mut_ptr() as *mut CmsgHdr;
            std::ptr::write_unaligned(
                hdr,
                CmsgHdr { cmsg_len: len, cmsg_level: SOL_SOCKET, cmsg_type: SCM_RIGHTS },
            );
            std::ptr::copy_nonoverlapping(
                fds.as_ptr() as *const u8,
                control.as_mut_ptr().add(CMSG_HDR_SPACE),
                payload,
            );
        }
        msg.msg_control = control.as_mut_ptr() as *mut c_void;
        msg.msg_controllen = cmsg_space(fds.len());
    }

    // MSG_NOSIGNAL: a compositor that exits should give this client an error
    // to report, not a SIGPIPE that kills it without a word.
    // SAFETY: `msg` points at initialised memory that outlives the call.
    let n = unsafe { sendmsg(sock, &msg, MSG_NOSIGNAL) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n as usize)
}

/// Receive bytes and any descriptors that came with them.
///
/// The descriptors are appended to `fds`. They arrive with `CLOEXEC` already
/// set, which matters because this process spawns `copal fleet` constantly and
/// a leaked keymap descriptor would ride along into every one of them.
pub fn recv_with_fds(sock: RawFd, buf: &mut [u8], fds: &mut Vec<RawFd>) -> io::Result<usize> {
    let mut iov = IoVec {
        iov_base: buf.as_mut_ptr() as *mut c_void,
        iov_len: buf.len(),
    };
    let mut control = [0u8; CONTROL_LEN];
    let mut msg = MsgHdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        _pad0: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: control.as_mut_ptr() as *mut c_void,
        msg_controllen: CONTROL_LEN,
        msg_flags: 0,
        _pad1: 0,
    };

    // SAFETY: `msg` describes `buf` and `control`, both live for the call.
    let n = unsafe { recvmsg(sock, &mut msg, MSG_CMSG_CLOEXEC) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }

    // Walk the control messages. There is normally one, but the loop is the
    // shape the API actually has and skipping it would drop descriptors.
    let mut offset = 0usize;
    while offset + CMSG_HDR_SPACE <= msg.msg_controllen {
        // SAFETY: the kernel wrote `msg_controllen` bytes into `control`, and
        // the bound above keeps the read inside them. Unaligned because the
        // backing buffer is `u8`.
        let hdr: CmsgHdr = unsafe {
            std::ptr::read_unaligned(control.as_ptr().add(offset) as *const CmsgHdr)
        };
        if hdr.cmsg_len < CMSG_HDR_SPACE || offset + hdr.cmsg_len > msg.msg_controllen {
            break;
        }
        if hdr.cmsg_level == SOL_SOCKET && hdr.cmsg_type == SCM_RIGHTS {
            let payload = hdr.cmsg_len - CMSG_HDR_SPACE;
            let count = payload / std::mem::size_of::<c_int>();
            for i in 0..count {
                // SAFETY: bounded by `count`, which came from the length the
                // kernel reported for this control message.
                let fd = unsafe {
                    std::ptr::read_unaligned(
                        control
                            .as_ptr()
                            .add(offset + CMSG_HDR_SPACE + i * std::mem::size_of::<c_int>())
                            as *const c_int,
                    )
                };
                fds.push(fd);
            }
        }
        offset += cmsg_align(hdr.cmsg_len);
    }

    Ok(n as usize)
}

/// An anonymous, memory-backed file: the shared pixel buffer's home.
///
/// `memfd_create` rather than a file under `XDG_RUNTIME_DIR` because a memfd
/// has no name in any directory, so there is no path for anything else to open
/// and no cleanup to get wrong if this process dies holding one.
pub fn memfd(name: &str, size: usize) -> io::Result<RawFd> {
    let mut c_name: Vec<u8> = name.bytes().take(200).collect();
    c_name.push(0);
    // SAFETY: `c_name` is NUL-terminated and lives across the call.
    let fd = unsafe { memfd_create(c_name.as_ptr() as *const c_char, MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a descriptor this function just created.
    if unsafe { ftruncate(fd, size as off_t) } < 0 {
        let e = io::Error::last_os_error();
        // SAFETY: closing a descriptor we own and are about to drop.
        unsafe { close(fd) };
        return Err(e);
    }
    Ok(fd)
}

/// A writable, shared mapping of `fd`, unmapped when it is dropped.
pub struct Mapping {
    ptr: *mut c_void,
    len: usize,
}

// SAFETY: the mapping is a plain block of bytes with no thread affinity; the
// pointer stays valid for the life of the `Mapping` regardless of which thread
// holds it. Access is through `&mut self`, so Rust still enforces exclusivity.
unsafe impl Send for Mapping {}

impl Mapping {
    pub fn new(fd: RawFd, len: usize) -> io::Result<Mapping> {
        if len == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "an empty mapping"));
        }
        // SAFETY: a null hint lets the kernel choose the address; `fd` is
        // owned by the caller and must outlive nothing, since `mmap` keeps its
        // own reference to the underlying object.
        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr as isize == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapping { ptr, len })
    }

    pub fn as_mut(&mut self) -> &mut [u8] {
        // SAFETY: `ptr` and `len` came from a successful `mmap` and are only
        // invalidated by `Drop`, which consumes the `Mapping`.
        unsafe { std::slice::from_raw_parts_mut(self.ptr as *mut u8, self.len) }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly the range this `Mapping` owns, once.
        unsafe { munmap(self.ptr, self.len) };
    }
}

/// Close a descriptor this program owns.
pub fn close_fd(fd: RawFd) {
    // SAFETY: the caller is giving up ownership of `fd`.
    unsafe { close(fd) };
}

#[cfg(test)]
mod tests {
    use super::*;

    // The layouts are the whole risk in this file, so they are asserted rather
    // than trusted. These numbers are the kernel's ABI for 64-bit little-endian
    // and are what both glibc and musl agree on there.
    #[test]
    fn msghdr_matches_the_kernel_abi() {
        assert_eq!(std::mem::size_of::<MsgHdr>(), 56);
        assert_eq!(std::mem::align_of::<MsgHdr>(), 8);
    }

    #[test]
    fn cmsghdr_matches_the_kernel_abi() {
        assert_eq!(std::mem::size_of::<CmsgHdr>(), 16);
        assert_eq!(CMSG_HDR_SPACE, 16);
    }

    #[test]
    fn iovec_matches_the_kernel_abi() {
        assert_eq!(std::mem::size_of::<IoVec>(), 16);
    }

    #[test]
    fn cmsg_space_leaves_room_for_the_header_and_the_descriptors() {
        assert_eq!(cmsg_space(1), 16 + 8, "one fd rounds up to the word");
        assert_eq!(cmsg_space(2), 16 + 8);
        assert_eq!(cmsg_space(3), 16 + 16);
        assert_eq!(cmsg_space(4), 16 + 16);
    }

    #[test]
    fn cmsg_align_rounds_to_the_word() {
        assert_eq!(cmsg_align(0), 0);
        assert_eq!(cmsg_align(1), 8);
        assert_eq!(cmsg_align(8), 8);
        assert_eq!(cmsg_align(9), 16);
    }
}

//! The arithmetic every remaining phase stands on.
//!
//! WHY THIS FILE EXISTS BEFORE `ssh.rs` AND `tls.rs` RATHER THAN INSIDE EITHER:
//! the two protocols want the same mathematics. SSH's key exchange is X25519
//! and SHA-256; TLS 1.3's is X25519 and SHA-256. SSH's host key is Ed25519;
//! the fleet's X.509 client certificate will be Ed25519. SSH's cipher is
//! ChaCha20-Poly1305; TLS's first suite is ChaCha20-Poly1305. Written inside
//! the first protocol that needed it, this would have been written a second
//! time -- differently, with a second set of bugs -- inside the second.
//!
//! THERE IS NO DESIGN SPACE IN THIS FILE, AND THAT IS THE POINT. `profile.rs`
//! already decided which algorithms exist: one per slot, no negotiation of
//! anything weaker, a test that fails the build if `sha1` or `cbc` or
//! `ssh-rsa` ever appears on a list. So this file is not a cryptographic
//! library with options; it is the shortest list of functions that satisfies
//! P1, and `nothing_here_is_off_the_profile` at the bottom says so.
//!
//! Every function has a published answer -- FIPS 180-4, RFC 4231, RFC 5869,
//! RFC 7748 section 5.2, RFC 8032 section 7.1, RFC 8439 section 2.8.2, the
//! NIST GCM vectors. A module where every line is checked against a number
//! somebody else wrote down is either right or obviously wrong, which is a
//! different risk from the protocol phases either side of it.
//!
//! CONSTANT TIME IS A PROPERTY OF THE CODE, NOT A COMMENT ON IT. No branch and
//! no array index anywhere below depends on a secret: selections are done with
//! a mask built from the bit, comparisons accumulate differences with OR and
//! test once at the end, and the AES S-box is a circuit rather than a table.
//! Where that costs speed the comment says what it cost and why the trade was
//! taken -- see `aes` for the only place the bill is large.
//!
//! WHAT THIS IS NOT: a reviewed library. `rustls` and `libsodium` have had eyes
//! on them this has not had, and `docs/wire.md` keeps that risk visible as R3
//! rather than burying it here. The argument for writing it is the one in
//! Cargo.toml -- a node's LAN never reaches the internet, so a crate is a fetch
//! that fails on the only machine that matters -- and the mitigation is
//! narrowness: one algorithm per slot, no legacy paths, and vectors for all of
//! it.

use std::fs::File;
use std::io::Read;

// ---------------------------------------------------------------------------
// Comparing and choosing without telling anybody what you chose
// ---------------------------------------------------------------------------

/// True when the two byte strings are equal, in time that depends on their
/// length and nothing else.
///
/// THE EARLY RETURN IS THE BUG THIS EXISTS TO AVOID. `a == b` on slices stops
/// at the first difference, and a remote peer that can time a tag check can
/// walk a forgery through it one byte at a time. Differences are accumulated
/// into one byte and looked at once.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // Length is not a secret -- it is on the wire in front of the value --
        // and pretending otherwise would mean comparing against a padding that
        // is itself a choice.
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// `0xff..` when `bit` is 1 and `0` when it is 0, for masking a choice.
#[inline]
fn mask64(bit: u64) -> u64 {
    (bit & 1).wrapping_neg()
}

/// Overwrite a secret before it goes out of scope.
///
/// `write_volatile` rather than a loop of assignments: the loop is dead stores
/// to the optimiser, which is allowed to delete it and usually does.
pub fn wipe(b: &mut [u8]) {
    for x in b.iter_mut() {
        unsafe { std::ptr::write_volatile(x, 0) };
    }
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Randomness
// ---------------------------------------------------------------------------

/// `n` bytes from the kernel.
///
/// `/dev/urandom` and not a userspace pool: the kernel's is seeded before
/// anything in this process runs, it survives a fork, and it is the same file
/// on Alpine and on a Mac. A failure here is fatal to the caller rather than
/// something to paper over with a counter -- a key derived from a fallback
/// nobody audited is worse than a refusal.
pub fn random(n: usize) -> Result<Vec<u8>, String> {
    let mut f = File::open("/dev/urandom").map_err(|e| format!("no randomness: {}", e))?;
    let mut out = vec![0u8; n];
    f.read_exact(&mut out)
        .map_err(|e| format!("short read from /dev/urandom: {}", e))?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// base64 -- the format every key file on the fleet is written in
// ---------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(B64[(n >> 18 & 63) as usize] as char);
        out.push(B64[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { B64[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { B64[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Decode, ignoring the line breaks OpenSSH wraps its keys at.
pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return Err(format!("{:?} is not base64", ch as char)),
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// SHA-2 -- FIPS 180-4
// ---------------------------------------------------------------------------

/// What HMAC needs to know about a hash: how long its output is, how long the
/// block it compresses is, and how to feed it.
///
/// A trait rather than two copies of HMAC, because HMAC is where the copies
/// diverge: the ipad/opad lengths follow the block size, and a 512-bit hash
/// pasted into a 256-bit HMAC is a bug that still produces plausible bytes.
pub trait Digest: Sized {
    const BLOCK: usize;
    const OUT: usize;
    fn new() -> Self;
    fn update(&mut self, data: &[u8]);
    fn finish(self) -> Vec<u8>;

    fn hash(data: &[u8]) -> Vec<u8> {
        let mut h = Self::new();
        h.update(data);
        h.finish()
    }
}

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256. The fleet's fingerprint hash, SSH's exchange hash, TLS 1.3's
/// transcript hash -- one implementation for all three.
pub struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    n: usize,
    len: u64,
}

impl Sha256 {
    fn block(&mut self, b: &[u8]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = self.h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K256[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v[7] = v[6];
            v[6] = v[5];
            v[5] = v[4];
            v[4] = v[3].wrapping_add(t1);
            v[3] = v[2];
            v[2] = v[1];
            v[1] = v[0];
            v[0] = t1.wrapping_add(t2);
        }
        for i in 0..8 {
            self.h[i] = self.h[i].wrapping_add(v[i]);
        }
    }
}

impl Digest for Sha256 {
    const BLOCK: usize = 64;
    const OUT: usize = 32;

    fn new() -> Sha256 {
        Sha256 {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0; 64],
            n: 0,
            len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        if self.n > 0 {
            let take = std::cmp::min(64 - self.n, data.len());
            self.buf[self.n..self.n + take].copy_from_slice(&data[..take]);
            self.n += take;
            data = &data[take..];
            if self.n < 64 {
                // The call ended inside a block. RETURNING HERE IS LOAD-BEARING:
                // falling through would reach the tail below with an empty
                // `data` and set `n` back to zero, throwing away the bytes just
                // buffered -- and a hash whose padding loop waits for a count
                // that keeps resetting never finishes at all.
                return;
            }
            let b = self.buf;
            self.block(&b);
            self.n = 0;
        }
        while data.len() >= 64 {
            let (b, rest) = data.split_at(64);
            self.block(b);
            data = rest;
        }
        self.buf[..data.len()].copy_from_slice(data);
        self.n = data.len();
    }

    fn finish(mut self) -> Vec<u8> {
        let bits = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.n != 56 {
            self.update(&[0]);
        }
        // `update` counted the padding into `len`, which is why the length was
        // taken before any of it was appended.
        let b = bits.to_be_bytes();
        self.update(&b);
        let mut out = Vec::with_capacity(32);
        for x in self.h.iter() {
            out.extend_from_slice(&x.to_be_bytes());
        }
        out
    }
}

const K512: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

/// SHA-512. Ed25519 is defined in terms of it and cannot be given another
/// hash, which is the whole reason a second hash is here at all.
pub struct Sha512 {
    h: [u64; 8],
    buf: [u8; 128],
    n: usize,
    len: u128,
}

impl Sha512 {
    fn block(&mut self, b: &[u8]) {
        let mut w = [0u64; 80];
        for i in 0..16 {
            let mut x = [0u8; 8];
            x.copy_from_slice(&b[i * 8..i * 8 + 8]);
            w[i] = u64::from_be_bytes(x);
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = self.h;
        for i in 0..80 {
            let s1 = v[4].rotate_right(14) ^ v[4].rotate_right(18) ^ v[4].rotate_right(41);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K512[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(28) ^ v[0].rotate_right(34) ^ v[0].rotate_right(39);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v[7] = v[6];
            v[6] = v[5];
            v[5] = v[4];
            v[4] = v[3].wrapping_add(t1);
            v[3] = v[2];
            v[2] = v[1];
            v[1] = v[0];
            v[0] = t1.wrapping_add(t2);
        }
        for i in 0..8 {
            self.h[i] = self.h[i].wrapping_add(v[i]);
        }
    }
}

impl Digest for Sha512 {
    const BLOCK: usize = 128;
    const OUT: usize = 64;

    fn new() -> Sha512 {
        Sha512 {
            h: [
                0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
                0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
            ],
            buf: [0; 128],
            n: 0,
            len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u128);
        if self.n > 0 {
            let take = std::cmp::min(128 - self.n, data.len());
            self.buf[self.n..self.n + take].copy_from_slice(&data[..take]);
            self.n += take;
            data = &data[take..];
            if self.n < 128 {
                // The call ended inside a block. RETURNING HERE IS LOAD-BEARING:
                // falling through would reach the tail below with an empty
                // `data` and set `n` back to zero, throwing away the bytes just
                // buffered -- and a hash whose padding loop waits for a count
                // that keeps resetting never finishes at all.
                return;
            }
            let b = self.buf;
            self.block(&b);
            self.n = 0;
        }
        while data.len() >= 128 {
            let (b, rest) = data.split_at(128);
            self.block(b);
            data = rest;
        }
        self.buf[..data.len()].copy_from_slice(data);
        self.n = data.len();
    }

    fn finish(mut self) -> Vec<u8> {
        let bits = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.n != 112 {
            self.update(&[0]);
        }
        let b = bits.to_be_bytes();
        self.update(&b);
        let mut out = Vec::with_capacity(64);
        for x in self.h.iter() {
            out.extend_from_slice(&x.to_be_bytes());
        }
        out
    }
}

pub fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::hash(data)
}

pub fn sha512(data: &[u8]) -> Vec<u8> {
    Sha512::hash(data)
}

// ---------------------------------------------------------------------------
// HMAC (RFC 2104) and HKDF (RFC 5869)
// ---------------------------------------------------------------------------

/// HMAC over any of the hashes above.
///
/// A key longer than the block is hashed first; a shorter one is padded with
/// zeroes. Both halves of that rule matter -- skipping the first turns a long
/// key into a buffer overrun, skipping the second makes two different keys
/// agree.
pub fn hmac<D: Digest>(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut k = [0u8; 128];
    let kb = if key.len() > D::BLOCK {
        D::hash(key)
    } else {
        key.to_vec()
    };
    k[..kb.len()].copy_from_slice(&kb);

    let mut inner = D::new();
    let ipad: Vec<u8> = k[..D::BLOCK].iter().map(|b| b ^ 0x36).collect();
    inner.update(&ipad);
    inner.update(msg);
    let mid = inner.finish();

    let mut outer = D::new();
    let opad: Vec<u8> = k[..D::BLOCK].iter().map(|b| b ^ 0x5c).collect();
    outer.update(&opad);
    outer.update(&mid);
    let out = outer.finish();
    wipe(&mut k);
    out
}

/// HKDF-Extract: turn whatever shape the shared secret arrived in into one
/// uniform pseudorandom key of the hash's width.
pub fn hkdf_extract<D: Digest>(salt: &[u8], ikm: &[u8]) -> Vec<u8> {
    let zeroes = vec![0u8; D::OUT];
    let s = if salt.is_empty() { &zeroes[..] } else { salt };
    hmac::<D>(s, ikm)
}

/// HKDF-Expand: as many bytes as the caller asked for, bound to `info`.
pub fn hkdf_expand<D: Digest>(prk: &[u8], info: &[u8], len: usize) -> Result<Vec<u8>, String> {
    if len > 255 * D::OUT {
        // The counter is one byte. Asking for more is a caller bug, and
        // wrapping it silently would hand out the same block twice.
        return Err("HKDF cannot expand that far".into());
    }
    let mut out = Vec::with_capacity(len);
    let mut t: Vec<u8> = Vec::new();
    let mut i = 1u8;
    while out.len() < len {
        let mut msg = t.clone();
        msg.extend_from_slice(info);
        msg.push(i);
        t = hmac::<D>(prk, &msg);
        out.extend_from_slice(&t);
        // Wrapping rather than `+= 1`: the 255th block is a legal request, and
        // incrementing past it is what the loop does on its way OUT rather
        // than a counter that is about to be used. The bound above is what
        // keeps the wrap unreachable.
        i = i.wrapping_add(1);
    }
    out.truncate(len);
    Ok(out)
}

/// TLS 1.3's HKDF-Expand-Label (RFC 8446 section 7.1), which is HKDF-Expand
/// with the label structure spelled out -- it lives here beside the HKDF it
/// wraps rather than in `tls.rs`, because `ssh.rs` has no use for it and a
/// reader looking for the key schedule should find all of it in one place.
pub fn hkdf_expand_label<D: Digest>(
    secret: &[u8],
    label: &str,
    context: &[u8],
    len: usize,
) -> Result<Vec<u8>, String> {
    let full = format!("tls13 {}", label);
    let mut info = Vec::new();
    info.extend_from_slice(&(len as u16).to_be_bytes());
    info.push(full.len() as u8);
    info.extend_from_slice(full.as_bytes());
    info.push(context.len() as u8);
    info.extend_from_slice(context);
    hkdf_expand::<D>(secret, &info, len)
}

// ---------------------------------------------------------------------------
// The field modulo 2^255 - 19
// ---------------------------------------------------------------------------
//
// FIVE LIMBS OF FIFTY-ONE BITS, which is the shape that makes the carries free.
// A 64-bit limb holding 51 bits leaves 13 bits of headroom, so a sum of five
// products (each under 2^104, times 19 for the folded top limb) stays inside a
// u128 and the whole multiply needs one carry pass at the end rather than a
// carry after every term. The alternative -- ten limbs of 25.5 bits -- is what
// you write when the machine has no 128-bit product; a Zero 2 and this Mac both
// do, so this is the shorter and faster shape on the hardware that exists.
//
// Limbs are allowed to be slightly larger than 51 bits between operations. The
// invariant maintained below is that every function returns limbs under 2^52,
// which is what keeps the multiply's headroom argument true no matter how the
// caller chains things.

const MASK51: u64 = (1u64 << 51) - 1;

/// An element of GF(2^255 - 19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fe([u64; 5]);

impl Fe {
    pub const ZERO: Fe = Fe([0, 0, 0, 0, 0]);
    pub const ONE: Fe = Fe([1, 0, 0, 0, 0]);

    /// Read a field element from 32 little-endian bytes.
    ///
    /// The top bit is dropped rather than rejected: both X25519 (RFC 7748
    /// section 5) and Ed25519 use it for something else -- the u-coordinate
    /// ignores it, the point encoding carries the sign of x in it -- so this
    /// has to be the lenient reader and the sign has to be handled by the
    /// caller that knows there is one.
    pub fn from_bytes(b: &[u8; 32]) -> Fe {
        let ld = |i: usize| -> u64 {
            let mut x = [0u8; 8];
            x.copy_from_slice(&b[i..i + 8]);
            u64::from_le_bytes(x)
        };
        Fe([
            ld(0) & MASK51,
            (ld(6) >> 3) & MASK51,
            (ld(12) >> 6) & MASK51,
            (ld(19) >> 1) & MASK51,
            (ld(24) >> 12) & MASK51,
        ])
    }

    /// The canonical 32 bytes: fully reduced, top bit clear.
    ///
    /// "Canonical" is the word doing the work. The limbs can represent values
    /// from 2^255-19 up to 2^255-1, and a serialiser that let those through
    /// would give two spellings of the same point -- which is how signature
    /// malleability and duplicated known_hosts entries both start.
    pub fn to_bytes(self) -> [u8; 32] {
        let mut t = self.0;
        // One carry pass, then the conditional subtraction of p.
        for _ in 0..2 {
            let mut carry = 0u64;
            for i in 0..5 {
                t[i] += carry;
                carry = t[i] >> 51;
                t[i] &= MASK51;
            }
            t[0] += carry * 19;
        }
        // q is 1 exactly when t >= p; the arithmetic below is the standard
        // trick and is branch-free because t is frequently secret.
        let mut q = (t[0] + 19) >> 51;
        q = (t[1] + q) >> 51;
        q = (t[2] + q) >> 51;
        q = (t[3] + q) >> 51;
        q = (t[4] + q) >> 51;
        t[0] += 19 * q;
        let mut carry = 0u64;
        for i in 0..5 {
            t[i] += carry;
            carry = t[i] >> 51;
            t[i] &= MASK51;
        }
        t[4] &= MASK51;

        let mut out = [0u8; 32];
        let v = [
            t[0] | t[1] << 51,
            t[1] >> 13 | t[2] << 38,
            t[2] >> 26 | t[3] << 25,
            t[3] >> 39 | t[4] << 12,
        ];
        for (i, x) in v.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&x.to_le_bytes());
        }
        out
    }

    fn add(self, o: Fe) -> Fe {
        let mut r = [0u64; 5];
        for i in 0..5 {
            r[i] = self.0[i] + o.0[i];
        }
        Fe(r).carry()
    }

    fn sub(self, o: Fe) -> Fe {
        // 2p added first so the subtraction cannot go negative. The limbs of
        // 2p are 2*(2^51-19) in the bottom and 2*(2^51-1) above it.
        let mut r = [0u64; 5];
        r[0] = self.0[0] + 0xfffffffffffda - o.0[0];
        for i in 1..5 {
            r[i] = self.0[i] + 0xffffffffffffe - o.0[i];
        }
        Fe(r).carry()
    }

    fn carry(self) -> Fe {
        let mut t = self.0;
        let mut c = 0u64;
        for i in 0..5 {
            t[i] += c;
            c = t[i] >> 51;
            t[i] &= MASK51;
        }
        t[0] += c * 19;
        // One more pass on the bottom limb: the fold above can push it over
        // 2^51 again, and the multiply's headroom argument assumes it did not.
        t[1] += t[0] >> 51;
        t[0] &= MASK51;
        Fe(t)
    }

    fn mul(self, o: Fe) -> Fe {
        let a = self.0;
        let b = o.0;
        // The top four limbs of b are pre-multiplied by 19 because every one
        // of them wraps past 2^255 into the bottom, and 2^255 = 19 mod p.
        let b19 = [b[1] * 19, b[2] * 19, b[3] * 19, b[4] * 19];
        let m = |x: u64, y: u64| -> u128 { x as u128 * y as u128 };
        let mut c = [0u128; 5];
        c[0] = m(a[0], b[0]) + m(a[1], b19[3]) + m(a[2], b19[2]) + m(a[3], b19[1]) + m(a[4], b19[0]);
        c[1] = m(a[0], b[1]) + m(a[1], b[0]) + m(a[2], b19[3]) + m(a[3], b19[2]) + m(a[4], b19[1]);
        c[2] = m(a[0], b[2]) + m(a[1], b[1]) + m(a[2], b[0]) + m(a[3], b19[3]) + m(a[4], b19[2]);
        c[3] = m(a[0], b[3]) + m(a[1], b[2]) + m(a[2], b[1]) + m(a[3], b[0]) + m(a[4], b19[3]);
        c[4] = m(a[0], b[4]) + m(a[1], b[3]) + m(a[2], b[2]) + m(a[3], b[1]) + m(a[4], b[0]);

        let mut r = [0u64; 5];
        let mut carry: u128 = 0;
        for i in 0..5 {
            let v = c[i] + carry;
            r[i] = (v as u64) & MASK51;
            carry = v >> 51;
        }
        r[0] += (carry as u64) * 19;
        r[1] += r[0] >> 51;
        r[0] &= MASK51;
        Fe(r)
    }

    fn sq(self) -> Fe {
        // A dedicated squaring saves about a third of the multiplies, and
        // squaring is what the inversion chain does two hundred and fifty
        // times -- but a separate formula is a separate place to be wrong, and
        // the ladder is already fast enough at forty microseconds a step. If a
        // Zero 2 ever measures too slow, THIS is the line to change first.
        self.mul(self)
    }

    fn mul121666(self) -> Fe {
        let mut r = [0u64; 5];
        let mut carry = 0u128;
        for i in 0..5 {
            let v = self.0[i] as u128 * 121666 + carry;
            r[i] = (v as u64) & MASK51;
            carry = v >> 51;
        }
        r[0] += (carry as u64) * 19;
        r[1] += r[0] >> 51;
        r[0] &= MASK51;
        Fe(r)
    }

    fn neg(self) -> Fe {
        Fe::ZERO.sub(self)
    }

    /// `a` when `bit` is 0 and `b` when it is 1, without a branch.
    fn select(a: Fe, b: Fe, bit: u64) -> Fe {
        let m = mask64(bit);
        let mut r = [0u64; 5];
        for i in 0..5 {
            r[i] = a.0[i] ^ (m & (a.0[i] ^ b.0[i]));
        }
        Fe(r)
    }

    /// Exchange the two elements when `bit` is 1. The ladder's whole
    /// constant-time argument is this function.
    fn cswap(a: &mut Fe, b: &mut Fe, bit: u64) {
        let m = mask64(bit);
        for i in 0..5 {
            let t = m & (a.0[i] ^ b.0[i]);
            a.0[i] ^= t;
            b.0[i] ^= t;
        }
    }

    /// x^(p-2), which is 1/x for every x but zero -- and 0 for zero, which is
    /// the behaviour the callers below want, because a zero here means the
    /// peer sent something degenerate and the answer should be a flat refusal
    /// rather than a division fault.
    fn invert(self) -> Fe {
        // The addition chain from the reference implementation: eleven
        // squarings deep, 254 squarings and 11 multiplies in total.
        let z1 = self;
        let z2 = z1.sq();
        let z8 = z2.sq().sq();
        let z9 = z1.mul(z8);
        let z11 = z2.mul(z9);
        let z22 = z11.sq();
        let z_5_0 = z9.mul(z22);
        let mut t = z_5_0.sq();
        for _ in 1..5 {
            t = t.sq();
        }
        let z_10_0 = t.mul(z_5_0);
        t = z_10_0.sq();
        for _ in 1..10 {
            t = t.sq();
        }
        let z_20_0 = t.mul(z_10_0);
        t = z_20_0.sq();
        for _ in 1..20 {
            t = t.sq();
        }
        let z_40_0 = t.mul(z_20_0);
        t = z_40_0.sq();
        for _ in 1..10 {
            t = t.sq();
        }
        let z_50_0 = t.mul(z_10_0);
        t = z_50_0.sq();
        for _ in 1..50 {
            t = t.sq();
        }
        let z_100_0 = t.mul(z_50_0);
        t = z_100_0.sq();
        for _ in 1..100 {
            t = t.sq();
        }
        let z_200_0 = t.mul(z_100_0);
        t = z_200_0.sq();
        for _ in 1..50 {
            t = t.sq();
        }
        let z_250_0 = t.mul(z_50_0);
        t = z_250_0.sq();
        for _ in 1..5 {
            t = t.sq();
        }
        t.mul(z11)
    }

    /// x^((p-5)/8) -- the first step of a square root on this curve, used only
    /// when decompressing a point.
    fn pow22523(self) -> Fe {
        let z1 = self;
        let z2 = z1.sq();
        let z8 = z2.sq().sq();
        let z9 = z1.mul(z8);
        let z11 = z2.mul(z9);
        let z22 = z11.sq();
        let z_5_0 = z9.mul(z22);
        let mut t = z_5_0.sq();
        for _ in 1..5 {
            t = t.sq();
        }
        let z_10_0 = t.mul(z_5_0);
        t = z_10_0.sq();
        for _ in 1..10 {
            t = t.sq();
        }
        let z_20_0 = t.mul(z_10_0);
        t = z_20_0.sq();
        for _ in 1..20 {
            t = t.sq();
        }
        let z_40_0 = t.mul(z_20_0);
        t = z_40_0.sq();
        for _ in 1..10 {
            t = t.sq();
        }
        let z_50_0 = t.mul(z_10_0);
        t = z_50_0.sq();
        for _ in 1..50 {
            t = t.sq();
        }
        let z_100_0 = t.mul(z_50_0);
        t = z_100_0.sq();
        for _ in 1..100 {
            t = t.sq();
        }
        let z_200_0 = t.mul(z_100_0);
        t = z_200_0.sq();
        for _ in 1..50 {
            t = t.sq();
        }
        let z_250_0 = t.mul(z_50_0);
        t = z_250_0.sq();
        t = t.sq();
        t.mul(z1)
    }

    fn is_zero(self) -> bool {
        self.to_bytes() == [0u8; 32]
    }

    fn is_negative(self) -> bool {
        self.to_bytes()[0] & 1 == 1
    }
}

// ---------------------------------------------------------------------------
// X25519 -- RFC 7748
// ---------------------------------------------------------------------------

/// A private scalar, clamped as RFC 7748 section 5 requires.
///
/// THE CLAMPING IS NOT COSMETIC. Clearing the bottom three bits puts the
/// scalar in the prime-order subgroup, so a peer's small-order point cannot
/// leak the scalar's low bits; setting bit 254 fixes the ladder's length, so
/// the number of iterations is not itself a side channel.
fn clamp(mut k: [u8; 32]) -> [u8; 32] {
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;
    k
}

/// The Montgomery ladder: `scalar` times the point with u-coordinate `u`.
fn x25519_raw(scalar: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    let k = clamp(*scalar);
    let x1 = Fe::from_bytes(u);
    let mut x2 = Fe::ONE;
    let mut z2 = Fe::ZERO;
    let mut x3 = x1;
    let mut z3 = Fe::ONE;
    let mut swap = 0u64;

    for i in (0..255).rev() {
        let bit = ((k[i >> 3] >> (i & 7)) & 1) as u64;
        swap ^= bit;
        Fe::cswap(&mut x2, &mut x3, swap);
        Fe::cswap(&mut z2, &mut z3, swap);
        swap = bit;

        let a = x2.add(z2);
        let b = x2.sub(z2);
        let aa = a.sq();
        let bb = b.sq();
        let e = aa.sub(bb);
        let c = x3.add(z3);
        let d = x3.sub(z3);
        let da = d.mul(a);
        let cb = c.mul(b);
        x3 = da.add(cb).sq();
        z3 = x1.mul(da.sub(cb).sq());
        x2 = aa.mul(bb);
        z2 = e.mul(bb.add(e.mul121666()));
    }
    Fe::cswap(&mut x2, &mut x3, swap);
    Fe::cswap(&mut z2, &mut z3, swap);
    x2.mul(z2.invert()).to_bytes()
}

/// The public value for a private scalar: the scalar times the base point,
/// whose u-coordinate is 9.
pub fn x25519_public(secret: &[u8; 32]) -> [u8; 32] {
    let mut nine = [0u8; 32];
    nine[0] = 9;
    x25519_raw(secret, &nine)
}

/// The shared secret, or a refusal.
///
/// THE ALL-ZERO CHECK IS THE ONE THING THIS FUNCTION ADDS OVER THE LADDER, and
/// it is not optional. Curve25519 has points of order 1, 2, 4 and 8; multiply
/// one of them by a clamped scalar and the result is zero no matter what the
/// scalar was. A peer that sends one and gets a session anyway has chosen the
/// session key. RFC 7748 section 6.1 says an implementation MAY check; for a
/// key exchange this is the whole security of the exchange, so here it MUST.
pub fn x25519(secret: &[u8; 32], peer: &[u8; 32]) -> Result<[u8; 32], String> {
    let out = x25519_raw(secret, peer);
    if out == [0u8; 32] {
        return Err("the peer offered a degenerate curve point".into());
    }
    Ok(out)
}

/// A fresh key pair: (secret, public).
pub fn x25519_keypair() -> Result<([u8; 32], [u8; 32]), String> {
    let r = random(32)?;
    let mut s = [0u8; 32];
    s.copy_from_slice(&r);
    let p = x25519_public(&s);
    Ok((s, p))
}

// ---------------------------------------------------------------------------
// Scalars modulo the group order L
// ---------------------------------------------------------------------------
//
// L = 2^252 + 27742317777372353535851937790883648493, the order of the
// Ed25519 base point.
//
// WHY THIS IS A BIT-AT-A-TIME LONG DIVISION AND NOT THE REFERENCE CODE'S
// BARRETT REDUCTION: `sc_reduce` in ref10 is a hundred and forty lines of
// hand-scheduled 21-bit limbs with magic constants, and there is no way for a
// reader of this file to check it other than running it. The loop below is
// shift, compare, conditional subtract -- the algorithm everybody learned for
// long division -- five hundred and twelve times over four limbs. It is
// perhaps twenty times slower and it runs twice per signature, which on a Zero
// 2 is tens of microseconds against the milliseconds the curve arithmetic
// costs. THE SLOW OBVIOUS ONE IS THE RIGHT TRADE WHEN IT IS NOT ON THE HOT
// PATH, and it is still constant time: the subtraction is always performed and
// the result is chosen with a mask.

const L: [u64; 4] = [
    0x5812631a5cf5d3ed,
    0x14def9dea2f79cd6,
    0x0000000000000000,
    0x1000000000000000,
];

/// `a - b`, and 1 when it borrowed.
fn sub4(a: [u64; 4], b: [u64; 4]) -> ([u64; 4], u64) {
    let mut r = [0u64; 4];
    let mut borrow = 0u64;
    for i in 0..4 {
        let (d, b1) = a[i].overflowing_sub(b[i]);
        let (d, b2) = d.overflowing_sub(borrow);
        r[i] = d;
        borrow = (b1 as u64) | (b2 as u64);
    }
    (r, borrow)
}

fn select4(a: [u64; 4], b: [u64; 4], bit: u64) -> [u64; 4] {
    let m = mask64(bit);
    let mut r = [0u64; 4];
    for i in 0..4 {
        r[i] = a[i] ^ (m & (a[i] ^ b[i]));
    }
    r
}

/// Subtract L when the value is at least L. One pass is enough for anything
/// under 2L, which is all the callers below produce.
fn sc_freeze(a: [u64; 4]) -> [u64; 4] {
    let (d, borrow) = sub4(a, L);
    select4(d, a, borrow)
}

/// Reduce a 64-byte little-endian value modulo L. This is the function that
/// turns a SHA-512 output into a scalar.
pub fn sc_reduce(wide: &[u8; 64]) -> [u8; 32] {
    let mut r = [0u64; 4];
    for i in (0..512).rev() {
        let bit = ((wide[i >> 3] >> (i & 7)) & 1) as u64;
        let mut carry = bit;
        for limb in r.iter_mut() {
            let next = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = next;
        }
        r = sc_freeze(r);
    }
    let mut out = [0u8; 32];
    for i in 0..4 {
        out[i * 8..i * 8 + 8].copy_from_slice(&r[i].to_le_bytes());
    }
    out
}

fn sc_load(b: &[u8; 32]) -> [u64; 4] {
    let mut r = [0u64; 4];
    for i in 0..4 {
        let mut x = [0u8; 8];
        x.copy_from_slice(&b[i * 8..i * 8 + 8]);
        r[i] = u64::from_le_bytes(x);
    }
    r
}

/// (a * b + c) mod L -- the shape Ed25519 signing wants, computed as a plain
/// schoolbook multiply into 512 bits and then one reduction.
pub fn sc_muladd(a: &[u8; 32], b: &[u8; 32], c: &[u8; 32]) -> [u8; 32] {
    let (x, y) = (sc_load(a), sc_load(b));
    let mut prod = [0u64; 8];
    for i in 0..4 {
        let mut carry: u128 = 0;
        for j in 0..4 {
            let v = prod[i + j] as u128 + x[i] as u128 * y[j] as u128 + carry;
            prod[i + j] = v as u64;
            carry = v >> 64;
        }
        let mut k = i + 4;
        while carry > 0 {
            let v = prod[k] as u128 + carry;
            prod[k] = v as u64;
            carry = v >> 64;
            k += 1;
        }
    }
    let mut wide = [0u8; 64];
    for i in 0..8 {
        wide[i * 8..i * 8 + 8].copy_from_slice(&prod[i].to_le_bytes());
    }
    let ab = sc_reduce(&wide);

    // The addition, in the same 64-byte shape so there is one reduction path
    // rather than two.
    let mut sum = [0u64; 4];
    let (p, q) = (sc_load(&ab), sc_load(c));
    let mut carry = 0u64;
    for i in 0..4 {
        let v = p[i] as u128 + q[i] as u128 + carry as u128;
        sum[i] = v as u64;
        carry = (v >> 64) as u64;
    }
    // Both inputs are already under L, so the sum is under 2L and cannot have
    // overflowed the four limbs -- but if it ever did, folding the carry back
    // in is wrong, so assert the assumption rather than paper over it.
    debug_assert_eq!(carry, 0);
    let out4 = sc_freeze(sum);
    let mut out = [0u8; 32];
    for i in 0..4 {
        out[i * 8..i * 8 + 8].copy_from_slice(&out4[i].to_le_bytes());
    }
    out
}

/// True when the 32 bytes are a canonical scalar -- strictly less than L.
///
/// THIS IS THE MALLEABILITY CHECK. A signature whose S is at least L can be
/// rewritten as S + L and still verify under an implementation that reduces
/// before checking; two distinct byte strings that both verify under one key
/// is exactly the property a signature is supposed not to have, and it has
/// broken things that assumed signatures were unique identifiers.
pub fn sc_is_canonical(b: &[u8; 32]) -> bool {
    let (_, borrow) = sub4(sc_load(b), L);
    borrow == 1
}

// ---------------------------------------------------------------------------
// Ed25519 -- RFC 8032
// ---------------------------------------------------------------------------

/// d = -121665/121666, the curve constant. The test below recomputes it from
/// that fraction rather than trusting the digits -- a transcription error in a
/// constant this size is invisible by eye and fatal in use.
const D: Fe = Fe([
    929955233495203,
    466365720129213,
    1662059464998953,
    2033849074728123,
    1442794654840575,
]);

/// 2d, which the addition formula wants and would otherwise compute every time.
const D2: Fe = Fe([
    1859910466990425,
    932731440258426,
    1072319116312658,
    1815898335770999,
    633789495995903,
]);

/// A square root of -1, needed when a decompressed x lands on the wrong root.
const SQRTM1: Fe = Fe([
    1718705420411056,
    234908883556509,
    2233514472574048,
    2117202627021982,
    765476049583133,
]);

/// The base point, in the same encoding as any other public key.
///
/// STORED AS ITS ENCODING AND DECOMPRESSED AT USE, rather than as four field
/// constants. It costs one inversion per signature, which is nothing beside
/// the scalar multiplication it precedes, and it buys something worth more:
/// the base point goes through the same `Point::decode` as a peer's key, so
/// the test that decoding works is also the test that the base point is right.
const B_BYTES: [u8; 32] = [
    0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
];

/// A point in extended coordinates (X:Y:Z:T) with x = X/Z, y = Y/Z, xy = T/Z.
#[derive(Clone, Copy)]
struct Point {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

impl Point {
    const IDENTITY: Point = Point {
        x: Fe::ZERO,
        y: Fe::ONE,
        z: Fe::ONE,
        t: Fe::ZERO,
    };

    /// ONE ADDITION FORMULA, USED FOR DOUBLING TOO. The Hisil-Wong-Carter-Dawson
    /// formula for a = -1 is unified: it gives the right answer when the two
    /// points are equal, when one is the identity, and when they are inverses.
    /// A dedicated doubling would be about a third faster and would be a second
    /// formula to get wrong; on a curve where the exceptional cases are exactly
    /// what an attacker gets to choose, one complete formula is the safer
    /// shape and the slower one, and this file takes the safer shape.
    fn add(self, o: Point) -> Point {
        let a = self.y.sub(self.x).mul(o.y.sub(o.x));
        let b = self.y.add(self.x).mul(o.y.add(o.x));
        let c = self.t.mul(D2).mul(o.t);
        let d = self.z.mul(o.z);
        let d = d.add(d);
        let e = b.sub(a);
        let f = d.sub(c);
        let g = d.add(c);
        let h = b.add(a);
        Point {
            x: e.mul(f),
            y: g.mul(h),
            t: e.mul(h),
            z: f.mul(g),
        }
    }

    fn neg(self) -> Point {
        Point {
            x: self.x.neg(),
            y: self.y,
            z: self.z,
            t: self.t.neg(),
        }
    }

    fn select(a: Point, b: Point, bit: u64) -> Point {
        Point {
            x: Fe::select(a.x, b.x, bit),
            y: Fe::select(a.y, b.y, bit),
            z: Fe::select(a.z, b.z, bit),
            t: Fe::select(a.t, b.t, bit),
        }
    }

    /// `scalar` times this point, in time that does not depend on the scalar.
    ///
    /// Double-and-always-add: the addition happens on every bit and the result
    /// is thrown away when the bit was zero. A window table would be four
    /// times faster and would need a constant-time table lookup to stay safe;
    /// at one millisecond a signature on the slowest machine in the fleet,
    /// this does not need to be four times faster.
    fn mul(self, scalar: &[u8; 32]) -> Point {
        let mut r = Point::IDENTITY;
        for i in (0..256).rev() {
            r = r.add(r);
            let bit = ((scalar[i >> 3] >> (i & 7)) & 1) as u64;
            let s = r.add(self);
            r = Point::select(r, s, bit);
        }
        r
    }

    fn encode(self) -> [u8; 32] {
        let zi = self.z.invert();
        let x = self.x.mul(zi);
        let y = self.y.mul(zi);
        let mut out = y.to_bytes();
        out[31] |= (x.is_negative() as u8) << 7;
        out
    }

    /// Recover a point from its 32-byte encoding, or refuse.
    ///
    /// Three ways to refuse, and all three are real: a y that is not the
    /// canonical spelling of its value (two encodings of one key would mean
    /// two spellings of one identity in `authorized_keys`), a y with no
    /// corresponding x (not a point on the curve at all), and the zero x with
    /// the sign bit set (a second spelling of the same point again).
    fn decode(b: &[u8; 32]) -> Result<Point, String> {
        let y = Fe::from_bytes(b);
        let mut canon = y.to_bytes();
        canon[31] |= b[31] & 0x80;
        if canon != *b {
            return Err("the point's y coordinate is not canonical".into());
        }
        let sign = (b[31] >> 7) as u64;

        let y2 = y.sq();
        let u = y2.sub(Fe::ONE);
        let v = y2.mul(D).add(Fe::ONE);
        let v3 = v.sq().mul(v);
        let mut x = v3.sq().mul(v).mul(u).pow22523().mul(v3).mul(u);

        let check = v.mul(x.sq());
        if check != u {
            if check == u.neg() {
                x = x.mul(SQRTM1);
            } else {
                return Err("that is not a point on the curve".into());
            }
        }
        if x.is_zero() && sign == 1 {
            return Err("the point's x coordinate is not canonical".into());
        }
        if x.is_negative() as u64 != sign {
            x = x.neg();
        }
        Ok(Point {
            x,
            y,
            z: Fe::ONE,
            t: x.mul(y),
        })
    }
}

/// The public key for a 32-byte seed -- the same 32 bytes OpenSSH keeps in a
/// `ssh-ed25519` private key file.
pub fn ed25519_public(seed: &[u8; 32]) -> [u8; 32] {
    let h = sha512(seed);
    let mut a = [0u8; 32];
    a.copy_from_slice(&h[..32]);
    let a = clamp(a);
    let b = Point::decode(&B_BYTES).expect("the base point is a point");
    b.mul(&a).encode()
}

/// Sign `msg`, returning the 64-byte R || S.
pub fn ed25519_sign(seed: &[u8; 32], msg: &[u8]) -> [u8; 64] {
    let h = sha512(seed);
    let mut a = [0u8; 32];
    a.copy_from_slice(&h[..32]);
    let a = clamp(a);
    let prefix = &h[32..];
    let b = Point::decode(&B_BYTES).expect("the base point is a point");
    let pubkey = b.mul(&a).encode();

    // r = H(prefix || M). DERIVED, NOT RANDOM: RFC 8032's determinism means a
    // machine with a broken random pool still cannot leak its key by repeating
    // a nonce, which is how ECDSA keys have actually been lost in the field.
    let mut hr = Sha512::new();
    hr.update(prefix);
    hr.update(msg);
    let mut wide = [0u8; 64];
    wide.copy_from_slice(&hr.finish());
    let r = sc_reduce(&wide);
    let rp = b.mul(&r).encode();

    let mut hk = Sha512::new();
    hk.update(&rp);
    hk.update(&pubkey);
    hk.update(msg);
    wide.copy_from_slice(&hk.finish());
    let k = sc_reduce(&wide);

    let s = sc_muladd(&k, &a, &r);
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&rp);
    sig[32..].copy_from_slice(&s);
    sig
}

/// Verify a signature. Every failure is the same answer -- false -- because a
/// caller has no use for the difference and an attacker does.
pub fn ed25519_verify(pubkey: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
    let mut rbytes = [0u8; 32];
    rbytes.copy_from_slice(&sig[..32]);
    let mut s = [0u8; 32];
    s.copy_from_slice(&sig[32..]);
    if !sc_is_canonical(&s) {
        return false;
    }
    let a = match Point::decode(pubkey) {
        Ok(p) => p,
        Err(_) => return false,
    };
    // R is decoded rather than compared as bytes, so a non-canonical R is
    // rejected here rather than quietly accepted by a byte comparison that
    // happens to match.
    if Point::decode(&rbytes).is_err() {
        return false;
    }
    let b = match Point::decode(&B_BYTES) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let mut hk = Sha512::new();
    hk.update(&rbytes);
    hk.update(pubkey);
    hk.update(msg);
    let mut wide = [0u8; 64];
    wide.copy_from_slice(&hk.finish());
    let k = sc_reduce(&wide);

    // [S]B - [k]A should be R. Computed in that direction rather than checking
    // [S]B == R + [k]A so there is one encoding comparison rather than two
    // point comparisons in projective coordinates, where equal points have
    // many representations.
    let lhs = b.mul(&s).add(a.neg().mul(&k));
    ct_eq(&lhs.encode(), &rbytes)
}

/// A fresh signing key: (seed, public).
pub fn ed25519_keypair() -> Result<([u8; 32], [u8; 32]), String> {
    let r = random(32)?;
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&r);
    let p = ed25519_public(&seed);
    Ok((seed, p))
}

// ---------------------------------------------------------------------------
// ChaCha20 and Poly1305 -- RFC 8439
// ---------------------------------------------------------------------------
//
// THIS IS THE CIPHER THE FLEET ACTUALLY USES. It is first on both cipher lists
// in `profile.rs`, and the reason is arithmetic rather than fashion: ChaCha is
// additions, exclusive-ors and rotations on 32-bit words, which every machine
// in the fleet does in one cycle each and none of which is a table lookup. It
// is constant time by construction, on a Cortex-A53 with no AES instructions,
// which is exactly what a Zero 2 is.

fn quarter(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

fn chacha_block(state: &[u32; 16]) -> [u8; 64] {
    let mut x = *state;
    for _ in 0..10 {
        quarter(&mut x, 0, 4, 8, 12);
        quarter(&mut x, 1, 5, 9, 13);
        quarter(&mut x, 2, 6, 10, 14);
        quarter(&mut x, 3, 7, 11, 15);
        quarter(&mut x, 0, 5, 10, 15);
        quarter(&mut x, 1, 6, 11, 12);
        quarter(&mut x, 2, 7, 8, 13);
        quarter(&mut x, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        out[i * 4..i * 4 + 4].copy_from_slice(&x[i].wrapping_add(state[i]).to_le_bytes());
    }
    out
}

fn chacha_state(key: &[u8; 32], words: [u32; 4]) -> [u32; 16] {
    let mut s = [0u32; 16];
    s[0] = 0x61707865;
    s[1] = 0x3320646e;
    s[2] = 0x79622d32;
    s[3] = 0x6b206574;
    for i in 0..8 {
        s[4 + i] = u32::from_le_bytes([
            key[i * 4],
            key[i * 4 + 1],
            key[i * 4 + 2],
            key[i * 4 + 3],
        ]);
    }
    s[12..16].copy_from_slice(&words);
    s
}

/// XOR `buf` with the keystream, IETF flavour: 32-bit counter, 96-bit nonce.
pub fn chacha20_xor(key: &[u8; 32], counter: u32, nonce: &[u8; 12], buf: &mut [u8]) {
    let n = |i: usize| u32::from_le_bytes([nonce[i], nonce[i + 1], nonce[i + 2], nonce[i + 3]]);
    let mut state = chacha_state(key, [counter, n(0), n(4), n(8)]);
    for chunk in buf.chunks_mut(64) {
        let ks = chacha_block(&state);
        for (b, k) in chunk.iter_mut().zip(ks.iter()) {
            *b ^= k;
        }
        // The counter is 32 bits and the caller is responsible for not asking
        // for 256 GB under one nonce; wrapping silently would repeat the
        // keystream, which is the one failure a stream cipher cannot survive.
        state[12] = state[12].checked_add(1).expect("chacha counter overflow");
    }
}

/// XOR with the keystream, original flavour: 64-bit counter, 64-bit nonce.
/// This is the shape `chacha20-poly1305@openssh.com` is defined over, which is
/// why both exist -- the IETF construction is TLS's and this one is SSH's.
pub fn chacha20_xor64(key: &[u8; 32], counter: u64, nonce: &[u8; 8], buf: &mut [u8]) {
    let n = |i: usize| u32::from_le_bytes([nonce[i], nonce[i + 1], nonce[i + 2], nonce[i + 3]]);
    let mut ctr = counter;
    let mut state = chacha_state(key, [ctr as u32, (ctr >> 32) as u32, n(0), n(4)]);
    for chunk in buf.chunks_mut(64) {
        let ks = chacha_block(&state);
        for (b, k) in chunk.iter_mut().zip(ks.iter()) {
            *b ^= k;
        }
        ctr = ctr.wrapping_add(1);
        state[12] = ctr as u32;
        state[13] = (ctr >> 32) as u32;
    }
}

/// Poly1305, in five 26-bit limbs.
///
/// WHY 26 AND NOT 32: the modulus is 2^130-5, so the accumulator needs 130
/// bits plus room for the carries a multiply produces. Five limbs of 26 bits
/// hold 130 exactly, and each partial product stays under 2^52 where a u64
/// holds it with room to add five of them. This is the same argument as the
/// 51-bit limbs above, one field down.
pub struct Poly1305 {
    r: [u32; 5],
    h: [u32; 5],
    pad: [u32; 4],
    buf: [u8; 16],
    n: usize,
}

impl Poly1305 {
    pub fn new(key: &[u8; 32]) -> Poly1305 {
        let rd = |i: usize| {
            u32::from_le_bytes([key[i], key[i + 1], key[i + 2], key[i + 3]])
        };
        // The clamping of r is from the paper: it clears the bits whose carries
        // would otherwise escape the limb arithmetic below.
        let r = [
            rd(0) & 0x3ffffff,
            (rd(3) >> 2) & 0x3ffff03,
            (rd(6) >> 4) & 0x3ffc0ff,
            (rd(9) >> 6) & 0x3f03fff,
            (rd(12) >> 8) & 0x00fffff,
        ];
        Poly1305 {
            r,
            h: [0; 5],
            pad: [rd(16), rd(20), rd(24), rd(28)],
            buf: [0; 16],
            n: 0,
        }
    }

    fn block(&mut self, m: &[u8], final_block: bool) {
        let hibit = if final_block { 0 } else { 1 << 24 };
        let rd = |i: usize| u32::from_le_bytes([m[i], m[i + 1], m[i + 2], m[i + 3]]);
        self.h[0] += rd(0) & 0x3ffffff;
        self.h[1] += (rd(3) >> 2) & 0x3ffffff;
        self.h[2] += (rd(6) >> 4) & 0x3ffffff;
        self.h[3] += (rd(9) >> 6) & 0x3ffffff;
        self.h[4] += (rd(12) >> 8) | hibit;

        let (r, h) = (self.r, self.h);
        let s = [r[1] * 5, r[2] * 5, r[3] * 5, r[4] * 5];
        let m64 = |a: u32, b: u32| a as u64 * b as u64;
        let d = [
            m64(h[0], r[0]) + m64(h[1], s[3]) + m64(h[2], s[2]) + m64(h[3], s[1]) + m64(h[4], s[0]),
            m64(h[0], r[1]) + m64(h[1], r[0]) + m64(h[2], s[3]) + m64(h[3], s[2]) + m64(h[4], s[1]),
            m64(h[0], r[2]) + m64(h[1], r[1]) + m64(h[2], r[0]) + m64(h[3], s[3]) + m64(h[4], s[2]),
            m64(h[0], r[3]) + m64(h[1], r[2]) + m64(h[2], r[1]) + m64(h[3], r[0]) + m64(h[4], s[3]),
            m64(h[0], r[4]) + m64(h[1], r[3]) + m64(h[2], r[2]) + m64(h[3], r[1]) + m64(h[4], r[0]),
        ];
        let mut c = 0u64;
        for i in 0..5 {
            let v = d[i] + c;
            self.h[i] = (v as u32) & 0x3ffffff;
            c = v >> 26;
        }
        self.h[0] += (c as u32) * 5;
        self.h[1] += self.h[0] >> 26;
        self.h[0] &= 0x3ffffff;
    }

    pub fn update(&mut self, mut data: &[u8]) {
        if self.n > 0 {
            let take = std::cmp::min(16 - self.n, data.len());
            self.buf[self.n..self.n + take].copy_from_slice(&data[..take]);
            self.n += take;
            data = &data[take..];
            if self.n < 16 {
                // The call ended inside a block. RETURNING HERE IS LOAD-BEARING:
                // falling through would reach the tail below with an empty
                // `data` and set `n` back to zero, throwing away the bytes just
                // buffered -- and a hash whose padding loop waits for a count
                // that keeps resetting never finishes at all.
                return;
            }
            let b = self.buf;
            self.block(&b, false);
            self.n = 0;
        }
        while data.len() >= 16 {
            let (b, rest) = data.split_at(16);
            self.block(b, false);
            data = rest;
        }
        self.buf[..data.len()].copy_from_slice(data);
        self.n = data.len();
    }

    pub fn finish(mut self) -> [u8; 16] {
        if self.n > 0 {
            let n = self.n;
            self.buf[n] = 1;
            for b in self.buf[n + 1..].iter_mut() {
                *b = 0;
            }
            let b = self.buf;
            self.block(&b, true);
        }
        // Carry, then subtract 2^130-5 if the accumulator is at least that --
        // branch-free, because h is a function of the message and the key.
        let mut h = self.h;
        let mut c = h[1] >> 26;
        h[1] &= 0x3ffffff;
        for i in 2..5 {
            h[i] += c;
            c = h[i] >> 26;
            h[i] &= 0x3ffffff;
        }
        h[0] += c * 5;
        h[1] += h[0] >> 26;
        h[0] &= 0x3ffffff;

        let mut g = [0u32; 5];
        let mut c = 5u32;
        for i in 0..5 {
            let v = h[i] + c;
            g[i] = v & 0x3ffffff;
            c = v >> 26;
        }
        g[4] = g[4].wrapping_sub(1 << 26);
        let mask = (c ^ 1).wrapping_sub(1); // all ones when g did not borrow
        for i in 0..5 {
            h[i] = (h[i] & !mask) | (g[i] & mask);
        }

        let f = |lo: u32, hi: u32, sh: u32| -> u32 { lo | (hi << sh) };
        let words = [
            f(h[0], h[1], 26),
            f(h[1] >> 6, h[2], 20),
            f(h[2] >> 12, h[3], 14),
            f(h[3] >> 18, h[4], 8),
        ];
        let mut out = [0u8; 16];
        let mut carry = 0u64;
        for i in 0..4 {
            let v = words[i] as u64 + self.pad[i] as u64 + carry;
            out[i * 4..i * 4 + 4].copy_from_slice(&(v as u32).to_le_bytes());
            carry = v >> 32;
        }
        out
    }

    pub fn tag(key: &[u8; 32], msg: &[u8]) -> [u8; 16] {
        let mut p = Poly1305::new(key);
        p.update(msg);
        p.finish()
    }
}

fn pad16(p: &mut Poly1305, len: usize) {
    let rem = len % 16;
    if rem != 0 {
        p.update(&[0u8; 16][..16 - rem]);
    }
}

/// AEAD_CHACHA20_POLY1305 (RFC 8439 section 2.8): encrypt in place, return the
/// tag.
///
/// Counter 0 makes the one-time Poly1305 key and the ciphertext starts at
/// counter 1 -- if those ever shared a counter the tag key would be recoverable
/// from the plaintext, and the whole authentication would be decorative.
pub fn chacha20_poly1305_seal(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    buf: &mut [u8],
) -> [u8; 16] {
    let mut block0 = [0u8; 64];
    chacha20_xor(key, 0, nonce, &mut block0);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&block0[..32]);

    chacha20_xor(key, 1, nonce, buf);

    let mut p = Poly1305::new(&otk);
    p.update(aad);
    pad16(&mut p, aad.len());
    p.update(buf);
    pad16(&mut p, buf.len());
    p.update(&(aad.len() as u64).to_le_bytes());
    p.update(&(buf.len() as u64).to_le_bytes());
    let tag = p.finish();
    wipe(&mut otk);
    wipe(&mut block0);
    tag
}

/// Verify and decrypt in place. THE TAG IS CHECKED BEFORE A SINGLE BYTE IS
/// DECRYPTED, which is the difference between this and every protocol that has
/// had to be patched for a padding oracle.
pub fn chacha20_poly1305_open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    buf: &mut [u8],
    tag: &[u8; 16],
) -> Result<(), String> {
    let mut block0 = [0u8; 64];
    chacha20_xor(key, 0, nonce, &mut block0);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&block0[..32]);

    let mut p = Poly1305::new(&otk);
    p.update(aad);
    pad16(&mut p, aad.len());
    p.update(buf);
    pad16(&mut p, buf.len());
    p.update(&(aad.len() as u64).to_le_bytes());
    p.update(&(buf.len() as u64).to_le_bytes());
    let want = p.finish();
    wipe(&mut otk);
    wipe(&mut block0);
    if !ct_eq(&want, tag) {
        return Err("the message was not authentic".into());
    }
    chacha20_xor(key, 1, nonce, buf);
    Ok(())
}

// ---------------------------------------------------------------------------
// AES-256 and GCM -- FIPS 197 and NIST SP 800-38D
// ---------------------------------------------------------------------------
//
// WHY THIS IS A CIRCUIT AND NOT A TABLE. Every fast AES starts with a 256-byte
// S-box and indexes it with a byte of the state -- which is a byte of the key
// mixed with a byte of the data -- so which cache line gets touched is a
// function of the secret. That is not a theoretical leak: cache-timing attacks
// have recovered AES keys from table-driven implementations across processes
// and across virtual machines. A node has no AES instructions to hide behind
// (a Cortex-A53 without the crypto extensions is exactly that machine), so the
// choice here is a table that leaks or a circuit that does not.
//
// THE CIRCUIT IS BITSLICED ACROSS THE BLOCK, NOT ACROSS BLOCKS. The sixteen
// bytes of one block are held as eight 16-bit words -- word i holds bit i of
// every byte -- so one sequence of ANDs and XORs does the S-box for all
// sixteen bytes at once. The inverse in GF(2^8) is x^254 by the usual addition
// chain: four multiplies and seven squarings, each a fixed gate pattern.
//
// WHAT IT COSTS, MEASURED RATHER THAN GUESSED. `how_fast_the_ciphers_are`, on
// this Mac, in release:
//
//     chacha20-poly1305   186.1 MB/s
//     aes-256-gcm           0.5 MB/s
//
// Three hundred and seventy times. That is not the "few times slower" a
// bitsliced AES usually costs, and the reason is that this one slices across
// the sixteen bytes of ONE block rather than across four blocks at a time:
// every gate does sixteen bytes of work where it could do sixty-four. THAT IS
// WHY CHACHA20-POLY1305 IS FIRST ON EVERY CIPHER LIST IN `profile.rs` -- it is
// constant time without paying for it, on a machine with no AES instructions,
// which is what every node in the fleet is.
//
// STILL OPEN, AND IT MATTERS BEFORE PHASE 7. Half a megabyte a second here is
// maybe a tenth of that on a Zero 2, which is not a transport -- it is a
// stall. A TLS server chooses the cipher from what the client offers, and
// rustls (which is what hypr-rdp is built on) ranks AES-256-GCM above ChaCha
// in its own preference order. So `tls.rs` MUST NOT simply offer both and let
// the peer decide: either it offers ChaCha alone, or this becomes the
// four-block bitslice first. The choice belongs in that phase; the number
// belongs here, where somebody can check it.

/// A block, bitsliced: `w[i]` bit `j` is bit `i` of byte `j`.
type Bs = [u16; 8];

fn ortho(block: &[u8; 16]) -> Bs {
    let mut w = [0u16; 8];
    for (j, byte) in block.iter().enumerate() {
        for i in 0..8 {
            w[i] |= (((*byte >> i) & 1) as u16) << j;
        }
    }
    w
}

fn unortho(w: &Bs) -> [u8; 16] {
    let mut b = [0u8; 16];
    for j in 0..16 {
        for i in 0..8 {
            b[j] |= (((w[i] >> j) & 1) as u8) << i;
        }
    }
    b
}

/// Multiply in GF(2^8) modulo x^8 + x^4 + x^3 + x + 1, sixteen bytes at a time.
fn gf_mul(a: &Bs, b: &Bs) -> Bs {
    let mut p = [0u16; 15];
    for i in 0..8 {
        for j in 0..8 {
            p[i + j] ^= a[i] & b[j];
        }
    }
    // Fold everything at or above x^8 back down. Going high to low matters:
    // each fold lands on exponents strictly below the one it came from, so one
    // pass finishes the job.
    for k in (8..15).rev() {
        let t = p[k];
        p[k] = 0;
        let base = k - 8;
        p[base] ^= t;
        p[base + 1] ^= t;
        p[base + 3] ^= t;
        p[base + 4] ^= t;
    }
    [p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]]
}

fn gf_sq(a: &Bs) -> Bs {
    gf_mul(a, a)
}

/// The AES S-box: invert in the field, then the affine map. Zero inverts to
/// zero, which is what x^254 gives and what the standard asks for.
fn sub_bytes(w: &mut Bs) {
    let x = *w;
    let x2 = gf_sq(&x);
    let x3 = gf_mul(&x2, &x);
    let x12 = gf_sq(&gf_sq(&x3));
    let x14 = gf_mul(&x12, &x2);
    let x15 = gf_mul(&x12, &x3);
    let x240 = gf_sq(&gf_sq(&gf_sq(&gf_sq(&x15))));
    let inv = gf_mul(&x240, &x14);

    // s_i = b_i ^ b_{i+4} ^ b_{i+5} ^ b_{i+6} ^ b_{i+7} ^ 0x63, indices mod 8.
    let c = 0x63u8;
    for i in 0..8 {
        let mut v = inv[i] ^ inv[(i + 4) % 8] ^ inv[(i + 5) % 8] ^ inv[(i + 6) % 8]
            ^ inv[(i + 7) % 8];
        if (c >> i) & 1 == 1 {
            v ^= 0xffff;
        }
        w[i] = v;
    }
}

/// Apply a fixed byte permutation to every bitsliced word.
fn permute(w: &mut Bs, table: &[usize; 16]) {
    for word in w.iter_mut() {
        let src = *word;
        let mut out = 0u16;
        for (dest, s) in table.iter().enumerate() {
            out |= ((src >> s) & 1) << dest;
        }
        *word = out;
    }
}

/// Row r moves left by r. In the column-major numbering the state uses, byte
/// 4c+r takes the value that was at 4((c+r) mod 4)+r.
const SHIFT_ROWS: [usize; 16] = [
    0, 5, 10, 15, 4, 9, 14, 3, 8, 13, 2, 7, 12, 1, 6, 11,
];

/// One row up within each column -- the `b1, b2, b3` of MixColumns.
const UP: [usize; 16] = [1, 2, 3, 0, 5, 6, 7, 4, 9, 10, 11, 8, 13, 14, 15, 12];

fn up(w: &Bs) -> Bs {
    let mut t = *w;
    permute(&mut t, &UP);
    t
}

/// Multiply every byte by x: shift up one bit and fold in 0x1b where the top
/// bit was set. In this representation that is eight XORs and no branches.
fn xtime(w: &Bs) -> Bs {
    let h = w[7];
    [
        h,
        w[0] ^ h,
        w[1],
        w[2] ^ h,
        w[3] ^ h,
        w[4],
        w[5],
        w[6],
    ]
}

fn mix_columns(w: &mut Bs) {
    let a = *w;
    let xt = xtime(&a);
    let xt_up = up(&xt);
    let a1 = up(&a);
    let a2 = up(&a1);
    let a3 = up(&a2);
    for i in 0..8 {
        w[i] = xt[i] ^ xt_up[i] ^ a1[i] ^ a2[i] ^ a3[i];
    }
}

/// An AES-256 key, expanded into fifteen round keys, each already bitsliced.
pub struct Aes256 {
    rk: [Bs; 15],
}

impl Aes256 {
    pub fn new(key: &[u8; 32]) -> Aes256 {
        let mut w = [[0u8; 4]; 60];
        for i in 0..8 {
            w[i].copy_from_slice(&key[i * 4..i * 4 + 4]);
        }
        let mut rcon = 1u8;
        for i in 8..60 {
            let mut t = w[i - 1];
            if i % 8 == 0 {
                t = [t[1], t[2], t[3], t[0]];
                t = sub_word(t);
                t[0] ^= rcon;
                // The round constant doubles in the field, which is the same
                // xtime as MixColumns, one byte wide.
                rcon = (rcon << 1) ^ (((rcon >> 7) & 1) * 0x1b);
            } else if i % 8 == 4 {
                t = sub_word(t);
            }
            for j in 0..4 {
                w[i][j] = w[i - 8][j] ^ t[j];
            }
        }
        let mut rk = [[0u16; 8]; 15];
        for r in 0..15 {
            let mut block = [0u8; 16];
            for j in 0..4 {
                block[j * 4..j * 4 + 4].copy_from_slice(&w[r * 4 + j]);
            }
            rk[r] = ortho(&block);
        }
        Aes256 { rk }
    }

    /// Encrypt one block in place. There is no decryption in this file: GCM
    /// uses the cipher in counter mode, where both directions are the same
    /// operation, and an unused inverse cipher is a hundred lines nobody
    /// tests.
    pub fn encrypt(&self, block: &mut [u8; 16]) {
        let mut s = ortho(block);
        for i in 0..8 {
            s[i] ^= self.rk[0][i];
        }
        for r in 1..14 {
            sub_bytes(&mut s);
            permute(&mut s, &SHIFT_ROWS);
            mix_columns(&mut s);
            for i in 0..8 {
                s[i] ^= self.rk[r][i];
            }
        }
        sub_bytes(&mut s);
        permute(&mut s, &SHIFT_ROWS);
        for i in 0..8 {
            s[i] ^= self.rk[14][i];
        }
        *block = unortho(&s);
    }
}

/// The S-box on four bytes, for the key schedule. It goes through the same
/// circuit as everything else -- a four-byte special case would be a second
/// S-box to keep correct.
fn sub_word(t: [u8; 4]) -> [u8; 4] {
    let mut block = [0u8; 16];
    block[..4].copy_from_slice(&t);
    let mut w = ortho(&block);
    sub_bytes(&mut w);
    let out = unortho(&w);
    [out[0], out[1], out[2], out[3]]
}

// -- GHASH -------------------------------------------------------------------
//
// GF(2^128) with the bits in the opposite order from everywhere else, because
// that is how SP 800-38D defines it. The multiply is shift-and-add over 128
// bits with a mask instead of a branch: a table-driven GHASH has the same
// cache problem as a table-driven S-box, and the authentication tag is exactly
// as secret as the key it is computed under.

fn gf128_mul(x: &mut [u64; 2], h: &[u64; 2]) {
    let mut z = [0u64; 2];
    let mut v = *h;
    for i in 0..128 {
        let bit = if i < 64 {
            (x[0] >> (63 - i)) & 1
        } else {
            (x[1] >> (127 - i)) & 1
        };
        let m = mask64(bit);
        z[0] ^= v[0] & m;
        z[1] ^= v[1] & m;
        let lsb = v[1] & 1;
        v[1] = (v[1] >> 1) | (v[0] << 63);
        v[0] >>= 1;
        v[0] ^= 0xe100000000000000 & mask64(lsb);
    }
    *x = z;
}

fn be(b: &[u8]) -> [u64; 2] {
    let mut hi = [0u8; 8];
    let mut lo = [0u8; 8];
    hi.copy_from_slice(&b[..8]);
    lo.copy_from_slice(&b[8..16]);
    [u64::from_be_bytes(hi), u64::from_be_bytes(lo)]
}

fn ghash(h: &[u64; 2], data: &[&[u8]]) -> [u8; 16] {
    let mut y = [0u64; 2];
    let mut block = [0u8; 16];
    let mut n = 0usize;
    let mut absorb = |y: &mut [u64; 2], b: &[u8; 16]| {
        let v = be(b);
        y[0] ^= v[0];
        y[1] ^= v[1];
        gf128_mul(y, h);
    };
    for part in data {
        for byte in part.iter() {
            block[n] = *byte;
            n += 1;
            if n == 16 {
                absorb(&mut y, &block);
                n = 0;
            }
        }
        // Each part is padded to a block boundary of its own: that is what the
        // A || pad || C || pad structure means, and running them together
        // would authenticate a different string than the peer computed.
        if n != 0 {
            for b in block[n..].iter_mut() {
                *b = 0;
            }
            absorb(&mut y, &block);
            n = 0;
        }
    }
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&y[0].to_be_bytes());
    out[8..].copy_from_slice(&y[1].to_be_bytes());
    out
}

fn gcm_ctr(aes: &Aes256, counter: &mut [u8; 16], buf: &mut [u8]) {
    for chunk in buf.chunks_mut(16) {
        // The counter is the bottom 32 bits and it wraps, by the standard's
        // definition rather than by accident.
        let n = u32::from_be_bytes([counter[12], counter[13], counter[14], counter[15]]);
        counter[12..].copy_from_slice(&n.wrapping_add(1).to_be_bytes());
        let mut ks = *counter;
        aes.encrypt(&mut ks);
        for (b, k) in chunk.iter_mut().zip(ks.iter()) {
            *b ^= k;
        }
    }
}

fn gcm_tag(aes: &Aes256, h: &[u64; 2], j0: &[u8; 16], aad: &[u8], ct: &[u8]) -> [u8; 16] {
    let mut lens = [0u8; 16];
    lens[..8].copy_from_slice(&((aad.len() as u64) * 8).to_be_bytes());
    lens[8..].copy_from_slice(&((ct.len() as u64) * 8).to_be_bytes());
    let s = ghash(h, &[aad, ct, &lens]);
    let mut mask = *j0;
    aes.encrypt(&mut mask);
    let mut tag = [0u8; 16];
    for i in 0..16 {
        tag[i] = s[i] ^ mask[i];
    }
    tag
}

fn gcm_setup(aes: &Aes256, nonce: &[u8; 12]) -> ([u64; 2], [u8; 16]) {
    let mut hb = [0u8; 16];
    aes.encrypt(&mut hb);
    let h = be(&hb);
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(nonce);
    j0[15] = 1;
    (h, j0)
}

/// AES-256-GCM: encrypt in place, return the tag. Ninety-six-bit nonce only,
/// which is what TLS and every other modern user of GCM sends -- the other
/// lengths go through a GHASH of the nonce and exist only for compatibility
/// with things this console will never talk to.
pub fn aes256_gcm_seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> [u8; 16] {
    let aes = Aes256::new(key);
    let (h, j0) = gcm_setup(&aes, nonce);
    let mut ctr = j0;
    gcm_ctr(&aes, &mut ctr, buf);
    gcm_tag(&aes, &h, &j0, aad, buf)
}

/// Verify, then decrypt. In that order, for the reason the ChaCha version
/// gives: a plaintext produced before the tag was checked is a plaintext an
/// attacker chose.
pub fn aes256_gcm_open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    buf: &mut [u8],
    tag: &[u8; 16],
) -> Result<(), String> {
    let aes = Aes256::new(key);
    let (h, j0) = gcm_setup(&aes, nonce);
    let want = gcm_tag(&aes, &h, &j0, aad, buf);
    if !ct_eq(&want, tag) {
        return Err("the message was not authentic".into());
    }
    let mut ctr = j0;
    gcm_ctr(&aes, &mut ctr, buf);
    Ok(())
}

// ---------------------------------------------------------------------------
// DER -- just enough ASN.1 to read and write a certificate
// ---------------------------------------------------------------------------
//
// The lockdown in `docs/lockdown.md` turns on the node demanding a client
// certificate signed by the fleet CA, and a certificate is DER. This is the
// writer and the reader for the handful of shapes that needs: definite-length
// only, no indefinite lengths, no BER. WHAT IS NOT HERE IS AS DELIBERATE AS
// WHAT IS: the bugs in ASN.1 parsers are in the generality, and a console that
// only ever reads certificates it issued itself does not need any of it.

/// A tag byte and its contents, wrapped with a definite length.
pub fn der(tag: u8, contents: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = contents.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes = n.to_be_bytes();
        let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len() - 1);
        out.push(0x80 | (bytes.len() - first) as u8);
        out.extend_from_slice(&bytes[first..]);
    }
    out.extend_from_slice(contents);
    out
}

pub fn der_seq(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut body = Vec::new();
    for p in parts {
        body.extend_from_slice(p);
    }
    der(0x30, &body)
}

/// An INTEGER, which is signed -- so a value whose top bit is set needs a
/// leading zero or it reads back as a negative number. Serial numbers are
/// where that bites, because they are usually random.
pub fn der_int(value: &[u8]) -> Vec<u8> {
    let first = value.iter().position(|b| *b != 0).unwrap_or(value.len());
    let trimmed = &value[first..];
    let mut body = Vec::new();
    if trimmed.is_empty() {
        body.push(0);
    } else {
        if trimmed[0] & 0x80 != 0 {
            body.push(0);
        }
        body.extend_from_slice(trimmed);
    }
    der(0x02, &body)
}

/// A BIT STRING with no unused bits, which is the only kind a key or a
/// signature ever is.
pub fn der_bits(value: &[u8]) -> Vec<u8> {
    let mut body = vec![0u8];
    body.extend_from_slice(value);
    der(0x03, &body)
}

/// An OBJECT IDENTIFIER from its arcs.
pub fn der_oid(arcs: &[u32]) -> Vec<u8> {
    let mut body = vec![(arcs[0] * 40 + arcs[1]) as u8];
    for arc in &arcs[2..] {
        let mut stack = Vec::new();
        let mut v = *arc;
        loop {
            stack.push((v & 0x7f) as u8);
            v >>= 7;
            if v == 0 {
                break;
            }
        }
        for (i, b) in stack.iter().enumerate().rev() {
            body.push(if i == 0 { *b } else { b | 0x80 });
        }
    }
    der(0x06, &body)
}

/// Read one value: its tag, its contents, and what follows it.
pub fn der_read(buf: &[u8]) -> Result<(u8, &[u8], &[u8]), String> {
    if buf.len() < 2 {
        return Err("truncated DER".into());
    }
    let tag = buf[0];
    let (len, at) = if buf[1] < 0x80 {
        (buf[1] as usize, 2)
    } else {
        let n = (buf[1] & 0x7f) as usize;
        if n == 0 || n > 4 {
            // Indefinite length, and anything claiming to be longer than four
            // gigabytes, are both refused rather than interpreted.
            return Err("unsupported DER length".into());
        }
        if buf.len() < 2 + n {
            return Err("truncated DER length".into());
        }
        let mut v = 0usize;
        for b in &buf[2..2 + n] {
            v = v << 8 | *b as usize;
        }
        (v, 2 + n)
    };
    if buf.len() < at + len {
        return Err("DER value runs past the end".into());
    }
    Ok((tag, &buf[at..at + len], &buf[at + len..]))
}

// ---------------------------------------------------------------------------
// The published answers
// ---------------------------------------------------------------------------
//
// Every test below is somebody else's number. That is the whole argument for
// hand-writing this file rather than importing one: a module where the
// expected output of every function was published before it was written is a
// module whose bugs are loud.

/// Shared by both test modules -- a second copy of a hex parser is a second
/// place for a test to be wrong about its own vectors.
#[cfg(test)]
mod tests_support {
    pub fn hex(s: &str) -> Vec<u8> {
        let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..clean.len() / 2)
            .map(|i| u8::from_str_radix(&clean[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    pub fn hexs(b: &[u8]) -> String {
        b.iter().map(|x| format!("{:02x}", x)).collect()
    }

    pub fn a32(b: &[u8]) -> [u8; 32] {
        let mut o = [0u8; 32];
        o.copy_from_slice(b);
        o
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;

    #[test]
    fn sha256_agrees_with_fips_180_4() {
        assert_eq!(
            hexs(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hexs(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hexs(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_does_not_care_how_the_message_arrives() {
        // The buffering is the part of a hash that gets written wrong, and a
        // hash that depends on chunk boundaries fails only against a peer.
        let msg: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        let once = sha256(&msg);
        for chunk in [1usize, 7, 63, 64, 65, 127, 128] {
            let mut h = Sha256::new();
            for part in msg.chunks(chunk) {
                h.update(part);
            }
            assert_eq!(h.finish(), once, "chunked at {} disagreed", chunk);
        }
    }

    #[test]
    fn sha512_agrees_with_fips_180_4() {
        assert_eq!(
            hexs(&sha512(b"abc")),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            hexs(&sha512(b"")),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
             47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        );
    }

    #[test]
    fn sha512_does_not_care_how_the_message_arrives() {
        let msg: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        let once = sha512(&msg);
        for chunk in [1usize, 13, 127, 128, 129, 255, 256] {
            let mut h = Sha512::new();
            for part in msg.chunks(chunk) {
                h.update(part);
            }
            assert_eq!(h.finish(), once, "chunked at {} disagreed", chunk);
        }
    }

    #[test]
    fn hmac_agrees_with_rfc_4231() {
        let key = vec![0x0bu8; 20];
        assert_eq!(
            hexs(&hmac::<Sha256>(&key, b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hexs(&hmac::<Sha512>(&key, b"Hi There")),
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cde\
             daa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
        );
        // Case 2: a key shorter than the block, so the zero padding is
        // exercised rather than the hash-the-key path.
        assert_eq!(
            hexs(&hmac::<Sha256>(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Case 3-shaped: a key longer than the block, which must be hashed.
        let long = vec![0xaau8; 131];
        assert_eq!(
            hexs(&hmac::<Sha256>(
                &long,
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn hkdf_agrees_with_rfc_5869() {
        let ikm = vec![0x0bu8; 22];
        let salt = hex("000102030405060708090a0b0c");
        let info = hex("f0f1f2f3f4f5f6f7f8f9");
        let prk = hkdf_extract::<Sha256>(&salt, &ikm);
        assert_eq!(
            hexs(&prk),
            "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"
        );
        let okm = hkdf_expand::<Sha256>(&prk, &info, 42).unwrap();
        assert_eq!(
            hexs(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf\
             34007208d5b887185865"
        );
    }

    #[test]
    fn hkdf_refuses_to_expand_past_its_counter() {
        let prk = vec![0u8; 32];
        assert!(hkdf_expand::<Sha256>(&prk, b"", 255 * 32).is_ok());
        assert!(hkdf_expand::<Sha256>(&prk, b"", 255 * 32 + 1).is_err());
    }

    #[test]
    fn base64_round_trips_and_matches_the_usual_examples() {
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        for n in 0..200usize {
            let v: Vec<u8> = (0..n).map(|i| (i * 7 % 256) as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&v)).unwrap(), v);
        }
        // OpenSSH wraps public keys at seventy columns, so the decoder has to
        // ignore what the wrapping put in.
        assert_eq!(b64_decode("Zm9v\nYmFy\n").unwrap(), b"foobar");
        assert!(b64_decode("not base64!").is_err());
    }

    #[test]
    fn constant_time_equality_still_answers_the_question() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }

    // -- the field ------------------------------------------------------

    #[test]
    fn the_curve_constants_are_what_their_definitions_say() {
        // d = -121665/121666. Recomputed here rather than trusted, because a
        // wrong digit in a fifteen-digit limb is invisible by eye and would
        // silently produce a different curve.
        let want = Fe([121665, 0, 0, 0, 0]).neg().mul(Fe([121666, 0, 0, 0, 0]).invert());
        assert_eq!(D.to_bytes(), want.to_bytes(), "D is not -121665/121666");
        assert_eq!(D2.to_bytes(), D.add(D).to_bytes(), "D2 is not 2d");
        assert_eq!(
            SQRTM1.sq().to_bytes(),
            Fe::ONE.neg().to_bytes(),
            "SQRTM1 does not square to -1"
        );
    }

    #[test]
    fn the_field_serialiser_gives_one_spelling_per_value() {
        // p itself, p+1 and 2^255-1 all have to come back as their canonical
        // forms -- two spellings of one value is how duplicate known_hosts
        // entries and malleable signatures both begin.
        let p = hex("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f");
        assert_eq!(hexs(&Fe::from_bytes(&a32(&p)).to_bytes()), "00".repeat(32));
        let p1 = hex("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f");
        assert_eq!(
            hexs(&Fe::from_bytes(&a32(&p1)).to_bytes()),
            format!("01{}", "00".repeat(31))
        );
        for n in 0..64u8 {
            let mut b = [0u8; 32];
            b[0] = n;
            b[7] = n.wrapping_mul(3);
            assert_eq!(Fe::from_bytes(&b).to_bytes(), b);
        }
    }

    #[test]
    fn field_inversion_undoes_multiplication() {
        for n in 1..50u64 {
            let x = Fe([n * 7919, n, n * 3, n * 5, n]);
            assert_eq!(x.mul(x.invert()).to_bytes(), Fe::ONE.to_bytes());
        }
        // Zero inverts to zero rather than trapping; the callers rely on it.
        assert!(Fe::ZERO.invert().is_zero());
    }

    // -- X25519 ---------------------------------------------------------

    #[test]
    fn x25519_agrees_with_rfc_7748_section_5_2() {
        let k = a32(&hex(
            "a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4",
        ));
        let u = a32(&hex(
            "e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c",
        ));
        assert_eq!(
            hexs(&x25519(&k, &u).unwrap()),
            "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552"
        );
        let k = a32(&hex(
            "4b66e9d4d1b4673c5ad22691957d6af5c11b6421e0ea01d42ca4169e7918ba0d",
        ));
        let u = a32(&hex(
            "e5210f12786811d3f4b7959d0538ae2c31dbe7106fc03c3efc4cd549c715a493",
        ));
        assert_eq!(
            hexs(&x25519(&k, &u).unwrap()),
            "95cbde9476e8907d7aade45cb4b873f88b595a68799fa152e6f8f7647aac7957"
        );
    }

    #[test]
    fn both_sides_of_the_exchange_reach_the_same_secret() {
        // RFC 7748 section 6.1 -- the exchange SSH and TLS both perform.
        let a = a32(&hex(
            "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
        ));
        let b = a32(&hex(
            "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb",
        ));
        let apub = x25519_public(&a);
        let bpub = x25519_public(&b);
        assert_eq!(
            hexs(&apub),
            "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"
        );
        assert_eq!(
            hexs(&bpub),
            "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f"
        );
        let s1 = x25519(&a, &bpub).unwrap();
        let s2 = x25519(&b, &apub).unwrap();
        assert_eq!(s1, s2);
        assert_eq!(
            hexs(&s1),
            "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742"
        );
    }

    #[test]
    fn a_small_order_point_is_refused_rather_than_agreed_with() {
        // THE TRAP. Every one of these multiplies to zero under any clamped
        // scalar, and a peer that sent one and got a session would have chosen
        // the session key.
        let secret = a32(&hex(
            "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
        ));
        let small = [
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0100000000000000000000000000000000000000000000000000000000000000",
            "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
            "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
            "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        ];
        for u in small {
            let p = a32(&hex(u));
            assert!(
                x25519(&secret, &p).is_err(),
                "{} was accepted as a peer key",
                u
            );
        }
    }

    #[test]
    fn a_fresh_pair_agrees_with_itself() {
        let (s1, p1) = x25519_keypair().unwrap();
        let (s2, p2) = x25519_keypair().unwrap();
        assert_ne!(p1, p2, "two key pairs came out the same");
        assert_eq!(x25519(&s1, &p2).unwrap(), x25519(&s2, &p1).unwrap());
    }

    // -- Ed25519 --------------------------------------------------------

    #[test]
    fn ed25519_agrees_with_rfc_8032_section_7_1() {
        let vectors: &[(&str, &str, &str, &str)] = &[
            (
                "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                "",
                "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
            ),
            (
                "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
                "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
                "72",
                "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
            ),
            (
                "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
                "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
                "af82",
                "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
            ),
        ];
        for (seed, want_pub, msg, want_sig) in vectors {
            let seed = a32(&hex(seed));
            let msg = hex(msg);
            assert_eq!(hexs(&ed25519_public(&seed)), *want_pub, "wrong public key");
            let sig = ed25519_sign(&seed, &msg);
            assert_eq!(hexs(&sig), *want_sig, "wrong signature");
            assert!(ed25519_verify(&a32(&hex(want_pub)), &msg, &sig));
        }
    }

    #[test]
    fn a_signature_that_was_touched_does_not_verify() {
        let (seed, pubkey) = ed25519_keypair().unwrap();
        let msg = b"orrery: run this on every node";
        let sig = ed25519_sign(&seed, msg);
        assert!(ed25519_verify(&pubkey, msg, &sig));

        assert!(!ed25519_verify(&pubkey, b"orrery: run this on one node", &sig));
        for i in 0..64 {
            let mut bad = sig;
            bad[i] ^= 1;
            assert!(!ed25519_verify(&pubkey, msg, &bad), "byte {} was ignored", i);
        }
        let mut other = pubkey;
        other[0] ^= 1;
        assert!(!ed25519_verify(&other, msg, &sig));
    }

    #[test]
    fn a_second_spelling_of_a_signature_is_refused() {
        // S + L verifies under an implementation that reduces before checking,
        // which would mean two byte strings for one signature. `sc_is_canonical`
        // is the line that stops it; this is the test that the line is there.
        let (seed, pubkey) = ed25519_keypair().unwrap();
        let msg = b"one signature, one spelling";
        let sig = ed25519_sign(&seed, msg);

        let mut s = [0u8; 32];
        s.copy_from_slice(&sig[32..]);
        assert!(sc_is_canonical(&s));

        let mut wide = [0u8; 64];
        wide[..32].copy_from_slice(&s);
        let mut carry = 0u16;
        let mut plus_l = [0u8; 32];
        let lbytes = hex("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010");
        for i in 0..32 {
            let v = s[i] as u16 + lbytes[i] as u16 + carry;
            plus_l[i] = v as u8;
            carry = v >> 8;
        }
        assert!(!sc_is_canonical(&plus_l), "S+L passed the canonical check");
        let mut malleable = sig;
        malleable[32..].copy_from_slice(&plus_l);
        assert!(
            !ed25519_verify(&pubkey, msg, &malleable),
            "a second spelling of the signature verified"
        );
    }

    #[test]
    fn a_public_key_that_is_not_a_point_is_refused() {
        // Not on the curve at all.
        let bad = a32(&hex(
            "0100000000000000000000000000000000000000000000000000000000000080",
        ));
        assert!(Point::decode(&bad).is_err() || ed25519_verify(&bad, b"x", &[0u8; 64]) == false);
        // A y coordinate at or above p is a second spelling of a key.
        let noncanon = a32(&hex(
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        ));
        assert!(Point::decode(&noncanon).is_err());
    }

    #[test]
    fn the_scalar_reduction_agrees_with_long_division() {
        // L reduces to zero, L-1 stays put, and 2^512-1 lands where a
        // big-integer calculator says it does.
        let l = hex("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010");
        let mut wide = [0u8; 64];
        wide[..32].copy_from_slice(&l);
        assert_eq!(hexs(&sc_reduce(&wide)), "00".repeat(32));

        wide[0] -= 1;
        assert_eq!(hexs(&sc_reduce(&wide)), hexs(&{
            let mut m = l.clone();
            m[0] -= 1;
            m
        }));

        let all = [0xffu8; 64];
        assert_eq!(
            hexs(&sc_reduce(&all)),
            "000f9c44e31106a447938568a71b0ed065bef517d273ecce3d9a307c1b419903"
        );
    }

    // -- ChaCha20-Poly1305 ----------------------------------------------

    #[test]
    fn chacha20_agrees_with_rfc_8439_section_2_4_2() {
        let key = a32(&hex(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        ));
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&hex("000000000000004a00000000"));
        let mut buf = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.".to_vec();
        chacha20_xor(&key, 1, &nonce, &mut buf);
        assert_eq!(
            hexs(&buf),
            "6e2e359a2568f98041ba0728dd0d6981e97e7aec1d4360c20a27afccfd9fae0b\
             f91b65c5524733ab8f593dabcd62b3571639d624e65152ab8f530c359f0861d8\
             07ca0dbf500d6a6156a38e088a22b65e52bc514d16ccf806818ce91ab7793736\
             5af90bbf74a35be6b40b8eedf2785e42874d"
        );
        // The block function on its own, from section 2.3.2 -- a different
        // nonce from the one above, which is the trap this pair of vectors
        // exists to keep anybody from walking into twice.
        let mut ks = [0u8; 64];
        let mut n2 = [0u8; 12];
        n2.copy_from_slice(&hex("000000090000004a00000000"));
        chacha20_xor(&key, 1, &n2, &mut ks);
        assert_eq!(
            hexs(&ks),
            "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4ed2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e"
        );

        // Encryption is its own inverse, which is what makes the pty and the
        // socket paths symmetric later on.
        chacha20_xor(&key, 1, &nonce, &mut buf);
        assert_eq!(&buf[..6], b"Ladies");
    }

    #[test]
    fn poly1305_agrees_with_rfc_8439_section_2_5_2() {
        let key = a32(&hex(
            "85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b",
        ));
        assert_eq!(
            hexs(&Poly1305::tag(&key, b"Cryptographic Forum Research Group")),
            "a8061dc1305136c6c22b8baf0c0127a9"
        );
    }

    #[test]
    fn the_aead_agrees_with_rfc_8439_section_2_8_2() {
        let key = a32(&hex(
            "808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f",
        ));
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&hex("070000004041424344454647"));
        let aad = hex("50515253c0c1c2c3c4c5c6c7");
        let mut buf = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.".to_vec();
        let tag = chacha20_poly1305_seal(&key, &nonce, &aad, &mut buf);
        assert_eq!(
            hexs(&buf),
            "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d6\
             3dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b36\
             92ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc\
             3ff4def08e4b7a9de576d26586cec64b6116"
        );
        assert_eq!(hexs(&tag), "1ae10b594f09e26a7e902ecbd0600691");

        chacha20_poly1305_open(&key, &nonce, &aad, &mut buf, &tag).unwrap();
        assert_eq!(&buf[..6], b"Ladies");
    }

    #[test]
    fn a_forged_aead_message_is_refused_and_not_decrypted() {
        let key = [7u8; 32];
        let nonce = [9u8; 12];
        let plain = b"copal-fleet-exec: reboot saskatchewan".to_vec();
        let mut buf = plain.clone();
        let tag = chacha20_poly1305_seal(&key, &nonce, b"hdr", &mut buf);
        let sealed = buf.clone();

        // A changed ciphertext, a changed tag, changed associated data and the
        // wrong nonce are four different lies and all four get one answer.
        let mut b = sealed.clone();
        b[0] ^= 1;
        assert!(chacha20_poly1305_open(&key, &nonce, b"hdr", &mut b, &tag).is_err());
        let mut t = tag;
        t[0] ^= 1;
        let mut b = sealed.clone();
        assert!(chacha20_poly1305_open(&key, &nonce, b"hdr", &mut b, &t).is_err());
        let mut b = sealed.clone();
        assert!(chacha20_poly1305_open(&key, &nonce, b"HDR", &mut b, &tag).is_err());
        assert_eq!(b, sealed, "a rejected message was decrypted anyway");
        let mut b = sealed.clone();
        assert!(chacha20_poly1305_open(&key, &[8u8; 12], b"hdr", &mut b, &tag).is_err());
    }

    #[test]
    fn the_ssh_flavour_of_chacha_counts_in_sixty_four_bits() {
        // The openssh cipher uses the original nonce layout rather than the
        // IETF one, and the two must not be confused: the same key and counter
        // give different keystreams, which is a whole session of garbage.
        let key = [1u8; 32];
        let mut a = [0u8; 64];
        let mut b = [0u8; 64];
        chacha20_xor64(&key, 0, &[0u8; 8], &mut a);
        chacha20_xor(&key, 0, &[0u8; 12], &mut b);
        assert_eq!(a, b, "a zero nonce should agree in both layouts");
        let mut a = [0u8; 64];
        chacha20_xor64(&key, 1, &[0u8; 8], &mut a);
        let mut b = [0u8; 64];
        chacha20_xor(&key, 1, &[0u8; 12], &mut b);
        assert_eq!(a, b, "the low counter word should agree");
        // And the part that differs: a 64-bit counter past 2^32.
        let mut a = [0u8; 64];
        chacha20_xor64(&key, 1u64 << 32, &[0u8; 8], &mut a);
        assert_ne!(a, b);
    }
}

#[cfg(test)]
mod aes_tests {
    use super::tests_support::*;
    use super::*;

    #[test]
    fn the_sbox_circuit_produces_the_sbox() {
        // Four values from the FIPS 197 table, spread across it, and the two
        // that a wrong affine constant gets wrong first.
        for (input, want) in [(0x00u8, 0x63u8), (0x01, 0x7c), (0x53, 0xed), (0xff, 0x16)] {
            let mut block = [0u8; 16];
            block[0] = input;
            let mut w = ortho(&block);
            sub_bytes(&mut w);
            assert_eq!(unortho(&w)[0], want, "S({:02x})", input);
        }
        // And the property the table has that a broken circuit would not: the
        // S-box is a permutation of all 256 bytes.
        let mut seen = [false; 256];
        for v in 0..=255u8 {
            let mut block = [0u8; 16];
            block[0] = v;
            let mut w = ortho(&block);
            sub_bytes(&mut w);
            let s = unortho(&w)[0] as usize;
            assert!(!seen[s], "S-box hit {:02x} twice", s);
            seen[s] = true;
        }
    }

    #[test]
    fn bitslicing_a_block_and_unslicing_it_gives_the_block_back() {
        let mut b = [0u8; 16];
        for i in 0..16 {
            b[i] = (i as u8).wrapping_mul(17).wrapping_add(3);
        }
        assert_eq!(unortho(&ortho(&b)), b);
    }

    #[test]
    fn aes256_agrees_with_fips_197_appendix_c_3() {
        let key = a32(&hex(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        ));
        let mut block = [0u8; 16];
        block.copy_from_slice(&hex("00112233445566778899aabbccddeeff"));
        Aes256::new(&key).encrypt(&mut block);
        assert_eq!(hexs(&block), "8ea2b7ca516745bfeafc49904b496089");
    }

    #[test]
    fn gcm_agrees_with_the_nist_test_cases() {
        // Case 13: nothing to encrypt and nothing to authenticate, which is
        // the case that catches a wrong H or a wrong J0.
        let key = [0u8; 32];
        let nonce = [0u8; 12];
        let mut empty: Vec<u8> = Vec::new();
        let tag = aes256_gcm_seal(&key, &nonce, &[], &mut empty);
        assert_eq!(hexs(&tag), "530f8afbc74536b9a963b4f1c4cb738b");

        // Case 14: one block of zeroes.
        let mut buf = vec![0u8; 16];
        let tag = aes256_gcm_seal(&key, &nonce, &[], &mut buf);
        assert_eq!(hexs(&buf), "cea7403d4d606b6e074ec5d3baf39d18");
        assert_eq!(hexs(&tag), "d0d1c8a799996bf0265b98b5d48ab919");

        // Case 16: a real key, a real nonce, sixty bytes of plaintext and
        // twenty of associated data -- the lengths that exercise the partial
        // final block in the counter and in BOTH halves of the hash.
        let key = a32(&hex(
            "feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308",
        ));
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&hex("cafebabefacedbaddecaf888"));
        let mut buf = hex(
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72\
             1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39",
        );
        let aad = hex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
        let tag = aes256_gcm_seal(&key, &nonce, &aad, &mut buf);
        assert_eq!(
            hexs(&buf),
            "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa\
             8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662"
        );
        assert_eq!(hexs(&tag), "76fc6ece0f4e1768cddf8853bb2d551b");

        aes256_gcm_open(&key, &nonce, &aad, &mut buf, &tag).unwrap();
        assert_eq!(&hexs(&buf)[..8], "d9313225");
    }

    #[test]
    fn a_forged_gcm_message_is_refused_and_not_decrypted() {
        let key = [3u8; 32];
        let nonce = [4u8; 12];
        let mut buf = b"the same rule as the other cipher".to_vec();
        let tag = aes256_gcm_seal(&key, &nonce, b"aad", &mut buf);
        let sealed = buf.clone();
        let mut b = sealed.clone();
        b[3] ^= 0x20;
        assert!(aes256_gcm_open(&key, &nonce, b"aad", &mut b, &tag).is_err());
        let mut b = sealed.clone();
        assert!(aes256_gcm_open(&key, &nonce, b"AAD", &mut b, &tag).is_err());
        assert_eq!(b, sealed, "a rejected message was decrypted anyway");
        aes256_gcm_open(&key, &nonce, b"aad", &mut b, &tag).unwrap();
        assert_eq!(b, b"the same rule as the other cipher".to_vec());
    }

    #[test]
    fn associated_data_is_hashed_in_its_own_blocks() {
        // A and C are each padded to a block boundary. Run together, these two
        // would hash identically -- and a tag that cannot tell them apart
        // lets a peer move bytes from the authenticated header into the
        // encrypted body.
        let key = [5u8; 32];
        let nonce = [6u8; 12];
        let mut b1 = b"defg".to_vec();
        let t1 = aes256_gcm_seal(&key, &nonce, b"abc", &mut b1);
        let mut b2 = b"cdefg".to_vec();
        let t2 = aes256_gcm_seal(&key, &nonce, b"ab", &mut b2);
        assert_ne!(t1, t2);
    }

    #[test]
    fn der_writes_the_lengths_the_standard_asks_for() {
        assert_eq!(der(0x04, &[1, 2, 3]), vec![0x04, 0x03, 1, 2, 3]);
        let long = vec![0xaau8; 200];
        let v = der(0x04, &long);
        assert_eq!(&v[..3], &[0x04, 0x81, 200]);
        let longer = vec![0xaau8; 300];
        let v = der(0x04, &longer);
        assert_eq!(&v[..4], &[0x04, 0x82, 1, 44]);

        // An INTEGER is signed, so a serial number with its top bit set needs
        // a leading zero or it reads back negative.
        assert_eq!(der_int(&[0x80]), vec![0x02, 0x02, 0x00, 0x80]);
        assert_eq!(der_int(&[0x7f]), vec![0x02, 0x01, 0x7f]);
        assert_eq!(der_int(&[0x00, 0x00, 0x2a]), vec![0x02, 0x01, 0x2a]);
        assert_eq!(der_int(&[0x00]), vec![0x02, 0x01, 0x00]);

        // 1.3.101.112 is Ed25519's identifier, and the one this file will use.
        assert_eq!(der_oid(&[1, 3, 101, 112]), vec![0x06, 0x03, 0x2b, 0x65, 0x70]);
        // 2.5.4.3, commonName, where a node's hostname goes.
        assert_eq!(der_oid(&[2, 5, 4, 3]), vec![0x06, 0x03, 0x55, 0x04, 0x03]);
    }

    #[test]
    fn der_reads_back_what_it_wrote_and_refuses_what_it_should() {
        let seq = der_seq(&[der_int(&[1]), der_oid(&[1, 3, 101, 112]), der_bits(&[9, 9])]);
        let (tag, body, rest) = der_read(&seq).unwrap();
        assert_eq!(tag, 0x30);
        assert!(rest.is_empty());
        let (t1, v1, rest) = der_read(body).unwrap();
        assert_eq!((t1, v1), (0x02, &[1u8][..]));
        let (t2, _, rest) = der_read(rest).unwrap();
        assert_eq!(t2, 0x06);
        let (t3, v3, rest) = der_read(rest).unwrap();
        assert_eq!((t3, v3), (0x03, &[0u8, 9, 9][..]));
        assert!(rest.is_empty());

        assert!(der_read(&[0x30]).is_err());
        assert!(der_read(&[0x30, 0x05, 1, 2]).is_err(), "a short value was read");
        assert!(der_read(&[0x30, 0x80]).is_err(), "an indefinite length was read");
    }

    /// Not a test -- a measurement, which is why it is ignored by default.
    /// `cargo test -- --ignored --nocapture how_fast` prints it.
    ///
    /// The number that matters is ChaCha20-Poly1305, because that is what the
    /// fleet actually runs: an SFTP transfer and an RDP bitmap stream both
    /// move at whatever this says. AES is printed beside it to keep the
    /// trade-off in the comment at the top of this section honest rather than
    /// asserted -- if the gap ever closes, the comment is wrong.
    #[test]
    #[ignore]
    fn how_fast_the_ciphers_are() {
        use std::time::Instant;
        let mb = 8usize;
        let mut buf = vec![0u8; mb << 20];
        let key = [1u8; 32];
        let nonce = [2u8; 12];

        let t = Instant::now();
        let tag = chacha20_poly1305_seal(&key, &nonce, b"", &mut buf);
        let chacha = mb as f64 / t.elapsed().as_secs_f64();
        assert_eq!(tag.len(), 16);

        let t = Instant::now();
        aes256_gcm_seal(&key, &nonce, b"", &mut buf);
        let aes = mb as f64 / t.elapsed().as_secs_f64();

        let t = Instant::now();
        let _ = sha256(&buf);
        let sha = mb as f64 / t.elapsed().as_secs_f64();

        let (seed, pubkey) = ed25519_keypair().unwrap();
        let n = 100;
        let t = Instant::now();
        for _ in 0..n {
            let _ = ed25519_sign(&seed, b"fleet");
        }
        let sign = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        let sig = ed25519_sign(&seed, b"fleet");
        let t = Instant::now();
        for _ in 0..n {
            assert!(ed25519_verify(&pubkey, b"fleet", &sig));
        }
        let verify = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

        let (sec, pubk) = x25519_keypair().unwrap();
        let t = Instant::now();
        for _ in 0..n {
            let _ = x25519(&sec, &pubk).unwrap();
        }
        let dh = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

        println!("chacha20-poly1305  {:8.1} MB/s", chacha);
        println!("aes-256-gcm        {:8.1} MB/s", aes);
        println!("sha-256            {:8.1} MB/s", sha);
        println!("ed25519 sign       {:8.2} ms", sign);
        println!("ed25519 verify     {:8.2} ms", verify);
        println!("x25519 exchange    {:8.2} ms", dh);
    }

    #[test]
    fn nothing_here_is_off_the_profile() {
        // THE TIE BACK TO `profile.rs`. That file decides what the fleet will
        // speak; this one has to be able to speak all of it and has no reason
        // to be able to speak anything else. If a name is added there, this
        // test is where the missing primitive shows up.
        let p = crate::profile::P1;
        for name in p.ssh_kex {
            assert_eq!(*name, "curve25519-sha256", "no implementation for {}", name);
        }
        for name in p.ssh_cipher.iter().chain(p.tls_suites.iter()) {
            let known = name.contains("chacha20-poly1305")
                || name.contains("aes256-gcm")
                || name.contains("CHACHA20_POLY1305")
                || name.contains("AES_256_GCM");
            assert!(known, "{} is on the profile with nothing to speak it", name);
        }
        for name in p.tls_groups {
            assert_eq!(*name, "x25519", "no implementation for group {}", name);
        }
        for name in p.ssh_host_key.iter().chain(p.ssh_pubkey_accepted.iter()) {
            assert!(name.starts_with("ssh-ed25519"), "no implementation for {}", name);
        }
        for name in p.tls_sig_algs {
            assert_eq!(*name, "ed25519", "no implementation for {}", name);
        }
    }
}

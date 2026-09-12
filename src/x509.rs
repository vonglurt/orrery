//! X.509, read narrowly: enough to check a node's certificate and no more.
//!
//! WHY THIS EXISTS AT ALL, GIVEN THE FLEET ALREADY HAS A CA. The fleet's
//! certificates are OpenSSH's format, which `ssh.rs` reads in ninety lines
//! because it is a flat list of fields. TLS does not accept those; RDP runs
//! over TLS; so a node's RDP server presents X.509 and this console has to be
//! able to read one. The alternative was not using TLS, and `docs/wire.md` §5
//! explains why there is no alternative to TLS under RDP.
//!
//! THE PARSER IS THE DANGEROUS PART OF ANY TLS CLIENT, and the mitigation here
//! is the same one the rest of the program uses: refuse almost everything.
//! One signature algorithm. One public-key algorithm. Definite lengths only.
//! No RSA, no ECDSA, no DSA, no certificate policies, no name constraints, no
//! CRL distribution points, no OCSP, no wildcard matching. Every one of those
//! is a feature the fleet does not use and a parser somebody has had a CVE in.
//!
//! WILDCARDS ARE REFUSED ON PURPOSE. `*.museum.local` would match a node this
//! console was not asked for, and the fleet issues one certificate per node
//! with that node's name in it. A wildcard in a fleet certificate is a mistake
//! at issue time and reads here as a name that does not match.

use crate::crypto;

/// Ed25519's identifier, 1.3.101.112 -- the only key and the only signature
/// this reader will accept.
const ED25519_OID: &[u8] = &[0x2b, 0x65, 0x70];
/// 2.5.4.3, commonName.
const CN_OID: &[u8] = &[0x55, 0x04, 0x03];
/// 2.5.29.17, subjectAltName.
const SAN_OID: &[u8] = &[0x55, 0x1d, 0x11];
/// 2.5.29.19, basicConstraints.
const BASIC_OID: &[u8] = &[0x55, 0x1d, 0x13];

/// One certificate, in the shape the checks below need.
#[derive(Debug, Clone)]
pub struct Cert {
    /// The whole thing, as it arrived. Kept because a chain is checked by
    /// signature over exactly these bytes.
    pub der: Vec<u8>,
    /// The bytes the signature covers: the TBSCertificate, verbatim.
    tbs: Vec<u8>,
    pub serial: Vec<u8>,
    pub issuer: String,
    pub subject: String,
    pub not_before: u64,
    pub not_after: u64,
    pub key: [u8; 32],
    /// Every dNSName in the subjectAltName, plus the common name if there was
    /// no SAN at all -- which is how certificates were named before 2000 and
    /// how a hand-made test certificate often still is.
    pub names: Vec<String>,
    pub is_ca: bool,
    signature: [u8; 64],
}

fn oid_is(body: &[u8], want: &[u8]) -> bool {
    body == want
}

/// The algorithm identifier, which must name Ed25519 and nothing else.
fn algorithm(der: &[u8]) -> Result<(), String> {
    let (tag, body, _) = crypto::der_read(der)?;
    if tag != 0x30 {
        return Err("an algorithm that is not a sequence".into());
    }
    let (t, oid, rest) = crypto::der_read(body)?;
    if t != 0x06 || !oid_is(oid, ED25519_OID) {
        return Err("this console only accepts ed25519 certificates".into());
    }
    // Ed25519 takes no parameters; an ABSENT parameter field is the correct
    // encoding and a NULL is the common wrong one. Both are tolerated here
    // because rejecting a NULL would refuse certificates several real tools
    // emit, and neither carries anything an attacker can use.
    if !rest.is_empty() {
        let (t, body, _) = crypto::der_read(rest)?;
        if !(t == 0x05 && body.is_empty()) {
            return Err("an ed25519 algorithm with parameters".into());
        }
    }
    Ok(())
}

/// Days from the civil calendar to the Unix epoch, by Howard Hinnant's
/// algorithm -- the same arithmetic `date` does, in twelve lines and with no
/// leap-year special cases written out.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// A UTCTime or a GeneralizedTime, as seconds since the epoch.
fn time(tag: u8, body: &[u8]) -> Result<u64, String> {
    let s = std::str::from_utf8(body).map_err(|_| "a time that is not text")?;
    let (year, rest) = match tag {
        0x17 if s.len() >= 13 => {
            let yy: i64 = s[0..2].parse().map_err(|_| "a year that is not a number")?;
            // RFC 5280: 00-49 is 2000-2049 and 50-99 is 1950-1999. A
            // certificate with a two-digit year is already a certificate
            // written before anybody expected this to still matter.
            (if yy < 50 { 2000 + yy } else { 1900 + yy }, &s[2..])
        }
        0x18 if s.len() >= 15 => {
            let y: i64 = s[0..4].parse().map_err(|_| "a year that is not a number")?;
            (y, &s[4..])
        }
        _ => return Err("a time in a shape this does not read".into()),
    };
    let num = |at: usize| -> Result<i64, String> {
        rest[at..at + 2]
            .parse::<i64>()
            .map_err(|_| "a date that is not a number".to_string())
    };
    let (mo, d, h, mi, sec) = (num(0)?, num(2)?, num(4)?, num(6)?, num(8)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return Err("a date nobody could mean".into());
    }
    let days = days_from_civil(year, mo, d);
    let secs = days * 86400 + h * 3600 + mi * 60 + sec;
    // BEFORE THE EPOCH IS ZERO, NOT A HUGE NUMBER. `as u64` on a negative
    // value wraps, and a notBefore in 1950 would then read as the year
    // 584 billion -- a certificate permanently "not valid yet", or worse, a
    // notAfter that never arrives. Zero means "valid since always", which is
    // what a 1950 notBefore was trying to say.
    Ok(secs.max(0) as u64)
}

/// The common name out of a Name, for showing a person which certificate this
/// was. NOT USED FOR MATCHING -- see `matches`.
fn common_name(name: &[u8]) -> String {
    let mut rest = name;
    while let Ok((tag, body, next)) = crypto::der_read(rest) {
        rest = next;
        if tag != 0x31 {
            continue;
        }
        let mut inner = body;
        while let Ok((t, pair, n)) = crypto::der_read(inner) {
            inner = n;
            if t != 0x30 {
                continue;
            }
            if let Ok((_, oid, after)) = crypto::der_read(pair) {
                if oid_is(oid, CN_OID) {
                    if let Ok((_, v, _)) = crypto::der_read(after) {
                        return String::from_utf8_lossy(v).to_string();
                    }
                }
            }
        }
    }
    String::new()
}

impl Cert {
    pub fn parse(der: &[u8]) -> Result<Cert, String> {
        let (tag, body, extra) = crypto::der_read(der)?;
        if tag != 0x30 {
            return Err("a certificate that is not a sequence".into());
        }
        if !extra.is_empty() {
            return Err("there is something after the certificate".into());
        }
        // The signature covers the TBSCertificate's own encoding, tag and
        // length included, so it is sliced out of the original bytes rather
        // than re-encoded. RE-ENCODING IS THE CLASSIC WAY TO BREAK THIS: a
        // parser that rebuilds what it read verifies a signature over its own
        // opinion of the certificate.
        let (t, tbs_body, after_tbs) = crypto::der_read(body)?;
        if t != 0x30 {
            return Err("a tbsCertificate that is not a sequence".into());
        }
        let tbs_len = body.len() - after_tbs.len();
        let tbs = body[..tbs_len].to_vec();

        algorithm(after_tbs)?;
        let (_, _, after_alg) = crypto::der_read(after_tbs)?;
        let (t, sig_bits, rest) = crypto::der_read(after_alg)?;
        if t != 0x03 || !rest.is_empty() {
            return Err("a signature that is not a bit string".into());
        }
        if sig_bits.len() != 65 || sig_bits[0] != 0 {
            return Err("an ed25519 signature is sixty-four bytes".into());
        }
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&sig_bits[1..]);

        // Inside the TBSCertificate.
        let mut c = tbs_body;
        let (t, _, next) = crypto::der_read(c)?;
        // [0] EXPLICIT version. v1 omits it, and v1 has no extensions, which
        // means no subjectAltName and no basicConstraints -- so a v1
        // certificate can never be a CA here and never names a node.
        if t == 0xa0 {
            c = next;
        }
        let (t, serial, next) = crypto::der_read(c)?;
        if t != 0x02 {
            return Err("a serial number that is not an integer".into());
        }
        c = next;
        algorithm(c)?;
        let (_, _, next) = crypto::der_read(c)?;
        c = next;
        let (_, issuer, next) = crypto::der_read(c)?;
        c = next;
        let (t, validity, next) = crypto::der_read(c)?;
        if t != 0x30 {
            return Err("a validity that is not a sequence".into());
        }
        c = next;
        let (t1, b1, v2) = crypto::der_read(validity)?;
        let (t2, b2, _) = crypto::der_read(v2)?;
        let not_before = time(t1, b1)?;
        let not_after = time(t2, b2)?;
        if not_after <= not_before {
            return Err("a certificate that expires before it starts".into());
        }
        let (_, subject, next) = crypto::der_read(c)?;
        c = next;

        // SubjectPublicKeyInfo.
        let (t, spki, next) = crypto::der_read(c)?;
        if t != 0x30 {
            return Err("a public key that is not a sequence".into());
        }
        algorithm(spki)?;
        let (_, _, after) = crypto::der_read(spki)?;
        let (t, bits, _) = crypto::der_read(after)?;
        if t != 0x03 || bits.len() != 33 || bits[0] != 0 {
            return Err("an ed25519 public key is thirty-two bytes".into());
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bits[1..]);
        c = next;

        // Extensions, if there are any.
        let mut names = Vec::new();
        let mut is_ca = false;
        while let Ok((t, body, next)) = crypto::der_read(c) {
            if t == 0xa3 {
                let (_, exts, _) = crypto::der_read(body)?;
                let mut e = exts;
                while let Ok((_, ext, n)) = crypto::der_read(e) {
                    e = n;
                    let (_, oid, after) = crypto::der_read(ext)?;
                    // The critical flag is optional and comes before the
                    // value; skipping it by position rather than by tag is how
                    // a parser reads the wrong field.
                    let mut v = after;
                    if let Ok((t, _, n2)) = crypto::der_read(v) {
                        if t == 0x01 {
                            v = n2;
                        }
                    }
                    let (_, value, _) = crypto::der_read(v)?;
                    if oid_is(oid, SAN_OID) {
                        let (_, list, _) = crypto::der_read(value)?;
                        let mut g = list;
                        while let Ok((t, name, n3)) = crypto::der_read(g) {
                            g = n3;
                            // [2] IMPLICIT IA5String is a dNSName. Everything
                            // else in there -- email, URI, IP -- is not a name
                            // this console matches on.
                            if t == 0x82 {
                                names.push(String::from_utf8_lossy(name).to_string());
                            }
                        }
                    } else if oid_is(oid, BASIC_OID) {
                        // BasicConstraints ::= SEQUENCE { cA BOOLEAN DEFAULT
                        // FALSE, pathLenConstraint INTEGER OPTIONAL }. The
                        // flag is INSIDE the sequence; reading the sequence's
                        // own tag as the boolean says every CA is not one,
                        // which is a chain that never verifies rather than one
                        // that wrongly does -- but it is still wrong.
                        if let Ok((t, seq, _)) = crypto::der_read(value) {
                            if t == 0x30 {
                                if let Ok((t, b, _)) = crypto::der_read(seq) {
                                    is_ca = t == 0x01 && b.first().copied().unwrap_or(0) != 0;
                                }
                            }
                        }
                    }
                }
            }
            c = next;
        }
        let subject_cn = common_name(subject);
        if names.is_empty() && !subject_cn.is_empty() {
            names.push(subject_cn.clone());
        }

        Ok(Cert {
            der: der.to_vec(),
            tbs,
            serial: serial.to_vec(),
            issuer: common_name(issuer),
            subject: subject_cn,
            not_before,
            not_after,
            key,
            names,
            is_ca,
            signature,
        })
    }

    /// Did `issuer` sign this certificate?
    pub fn signed_by(&self, issuer: &[u8; 32]) -> bool {
        crypto::ed25519_verify(issuer, &self.tbs, &self.signature)
    }

    /// Does this certificate name that host?
    ///
    /// Exact, case-insensitive, no wildcards. A certificate for a node names
    /// that node.
    pub fn matches(&self, host: &str) -> bool {
        let want = host.trim_end_matches('.').to_ascii_lowercase();
        self.names
            .iter()
            .any(|n| n.trim_end_matches('.').to_ascii_lowercase() == want)
    }
}

/// Check a chain: leaf first, as TLS sends it, against the trusted keys.
///
/// THE ORDER OF THE CHECKS IS THE SAME ONE `ssh.rs` USES and for the same
/// reason: signatures first, names and dates afterwards, because until the
/// signature is checked every field is text an attacker chose. The chain is
/// walked from the leaf upwards, each link verified by the next, and the top
/// link has to be signed by a key this fleet knows.
pub fn verify(chain: &[Cert], trusted: &[[u8; 32]], host: &str, now: u64) -> Result<(), String> {
    let leaf = chain.first().ok_or("the node sent no certificate")?;
    if trusted.is_empty() {
        return Err("there is no certificate authority to check against".into());
    }

    let mut i = 0;
    loop {
        let cert = &chain[i];
        // A trusted key ends the walk wherever it appears, so a chain that
        // includes the root as its last certificate works and so does one that
        // stops at an intermediate this fleet happens to trust directly.
        if trusted.iter().any(|k| cert.signed_by(k)) {
            break;
        }
        let next = chain.get(i + 1).ok_or_else(|| {
            format!(
                "the certificate for {} was signed by {}, which this fleet does not know",
                if cert.subject.is_empty() { "the node" } else { &cert.subject },
                if cert.issuer.is_empty() { "somebody" } else { &cert.issuer },
            )
        })?;
        if !next.is_ca {
            return Err("a certificate in the chain is not a certificate authority".into());
        }
        if !cert.signed_by(&next.key) {
            return Err("the chain does not hold together".into());
        }
        i += 1;
        if i > 8 {
            return Err("a certificate chain longer than anybody means".into());
        }
    }

    // Now the fields, which are worth reading because the signature held.
    for cert in &chain[..=i] {
        if now < cert.not_before {
            return Err(format!(
                "the certificate for {} is not valid yet",
                cert.subject
            ));
        }
        if now > cert.not_after {
            return Err(format!("the certificate for {} has expired", cert.subject));
        }
    }
    if !leaf.matches(host) {
        return Err(format!(
            "the certificate is for {} and this is {}",
            if leaf.names.is_empty() {
                "nothing in particular".to_string()
            } else {
                leaf.names.join(", ")
            },
            host
        ));
    }
    Ok(())
}

/// Pull every certificate out of a PEM file.
pub fn from_pem(text: &str) -> Result<Vec<Cert>, String> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(BEGIN) {
        let after = &rest[at + BEGIN.len()..];
        let end = after.find(END).ok_or("a PEM certificate with no end")?;
        out.push(Cert::parse(&crypto::b64_decode(&after[..end])?)?);
        rest = &after[end..];
    }
    if out.is_empty() {
        return Err("that file has no certificate in it".into());
    }
    Ok(out)
}

/// The Ed25519 seed out of a PKCS#8 private key file.
///
/// One shape only: version 0, algorithm ed25519, and an OCTET STRING holding
/// the DER OCTET STRING that holds the seed. Encrypted keys are refused for
/// the reason `ssh.rs` gives about passphrases.
pub fn key_from_pem(text: &str) -> Result<[u8; 32], String> {
    const BEGIN: &str = "-----BEGIN PRIVATE KEY-----";
    const END: &str = "-----END PRIVATE KEY-----";
    let at = text
        .find(BEGIN)
        .ok_or("that is not an unencrypted PKCS#8 private key")?;
    let after = &text[at + BEGIN.len()..];
    let end = after.find(END).ok_or("the private key has no end")?;
    let der = crypto::b64_decode(&after[..end])?;

    let (t, body, _) = crypto::der_read(&der)?;
    if t != 0x30 {
        return Err("a private key that is not a sequence".into());
    }
    let (t, version, after_v) = crypto::der_read(body)?;
    if t != 0x02 || version != [0] {
        return Err("a private key version this does not read".into());
    }
    algorithm(after_v)?;
    let (_, _, after_alg) = crypto::der_read(after_v)?;
    let (t, wrapped, _) = crypto::der_read(after_alg)?;
    if t != 0x04 {
        return Err("a private key that is not an octet string".into());
    }
    let (t, seed, _) = crypto::der_read(wrapped)?;
    if t != 0x04 || seed.len() != 32 {
        return Err("an ed25519 seed is thirty-two bytes".into());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(seed);
    Ok(out)
}

/// Making certificates, for the tests and for nobody else.
///
/// THE CONSOLE NEVER ISSUES A CERTIFICATE -- that is the CA's job and
/// `docs/lockdown.md` draws the line. This module exists so the reader above
/// can be tested against certificates with deliberate faults in them: expired,
/// self-signed, named for another node, signed by a stranger. `tools/tls-check.sh`
/// checks the same reader against certificates OpenSSL made, which is the half
/// this cannot prove.
#[cfg(test)]
pub mod mint {
    use super::*;

    /// The inverse of `days_from_civil`, for writing a UTCTime.
    fn civil_from_days(z: i64) -> (i64, i64, i64) {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (if m <= 2 { y + 1 } else { y }, m, d)
    }

    fn utc_time(at: u64) -> Vec<u8> {
        let days = (at / 86400) as i64;
        let rest = at % 86400;
        let (y, m, d) = civil_from_days(days);
        let text = format!(
            "{:02}{:02}{:02}{:02}{:02}{:02}Z",
            y % 100,
            m,
            d,
            rest / 3600,
            rest % 3600 / 60,
            rest % 60
        );
        crypto::der(0x17, text.as_bytes())
    }

    fn name(cn: &str) -> Vec<u8> {
        let pair = crypto::der_seq(&[crypto::der_oid(&[2, 5, 4, 3]), crypto::der(0x0c, cn.as_bytes())]);
        crypto::der_seq(&[crypto::der(0x31, &pair)])
    }

    fn ed25519_alg() -> Vec<u8> {
        crypto::der_seq(&[crypto::der_oid(&[1, 3, 101, 112])])
    }

    pub struct Spec<'a> {
        pub subject: &'a str,
        pub issuer: &'a str,
        pub names: &'a [&'a str],
        pub not_before: u64,
        pub not_after: u64,
        pub is_ca: bool,
    }

    /// Build and sign one certificate.
    pub fn make(spec: &Spec, key: &[u8; 32], signer_seed: &[u8; 32]) -> Vec<u8> {
        let mut exts = Vec::new();
        if !spec.names.is_empty() {
            let mut list = Vec::new();
            for n in spec.names {
                list.extend_from_slice(&crypto::der(0x82, n.as_bytes()));
            }
            let inner = crypto::der(0x30, &list);
            exts.push(crypto::der_seq(&[
                crypto::der_oid(&[2, 5, 29, 17]),
                crypto::der(0x04, &inner),
            ]));
        }
        if spec.is_ca {
            let bc = crypto::der_seq(&[crypto::der(0x01, &[0xff])]);
            exts.push(crypto::der_seq(&[
                crypto::der_oid(&[2, 5, 29, 19]),
                crypto::der(0x01, &[0xff]),
                crypto::der(0x04, &bc),
            ]));
        }
        let extensions = crypto::der(0xa3, &crypto::der(0x30, &exts.concat()));

        let tbs = crypto::der_seq(&[
            crypto::der(0xa0, &crypto::der_int(&[2])),
            crypto::der_int(&[0x2a]),
            ed25519_alg(),
            name(spec.issuer),
            crypto::der_seq(&[utc_time(spec.not_before), utc_time(spec.not_after)]),
            name(spec.subject),
            crypto::der_seq(&[ed25519_alg(), crypto::der_bits(key)]),
            extensions,
        ]);
        let sig = crypto::ed25519_sign(signer_seed, &tbs);
        crypto::der_seq(&[tbs, ed25519_alg(), crypto::der_bits(&sig)])
    }
}

#[cfg(test)]
mod tests {
    use super::mint::{make, Spec};
    use super::*;

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    struct Fleet {
        ca_seed: [u8; 32],
        ca_key: [u8; 32],
        node_seed: [u8; 32],
        node_key: [u8; 32],
    }

    fn fleet() -> Fleet {
        let (ca_seed, ca_key) = crypto::ed25519_keypair().unwrap();
        let (node_seed, node_key) = crypto::ed25519_keypair().unwrap();
        Fleet { ca_seed, ca_key, node_seed, node_key }
    }

    fn node_cert(f: &Fleet, host: &str, from: i64, to: i64) -> Cert {
        let n = now() as i64;
        let der = make(
            &Spec {
                subject: host,
                issuer: "copal fleet CA",
                names: &[host],
                not_before: (n + from) as u64,
                not_after: (n + to) as u64,
                is_ca: false,
            },
            &f.node_key,
            &f.ca_seed,
        );
        Cert::parse(&der).expect("a certificate this file made should parse")
    }

    #[test]
    fn a_certificate_reads_back_the_way_it_was_written() {
        let f = fleet();
        let c = node_cert(&f, "museum-01", -60, 3600);
        assert_eq!(c.subject, "museum-01");
        assert_eq!(c.issuer, "copal fleet CA");
        assert_eq!(c.names, vec!["museum-01"]);
        assert_eq!(c.key, f.node_key);
        assert!(!c.is_ca);
        assert!(c.not_after > c.not_before);
        assert!(c.signed_by(&f.ca_key), "the CA's own signature did not check");

        let (_, other) = crypto::ed25519_keypair().unwrap();
        assert!(!c.signed_by(&other));
    }

    #[test]
    fn every_way_a_node_can_fail_the_check_is_a_sentence() {
        let f = fleet();
        let good = node_cert(&f, "museum-01", -60, 3600);
        verify(&[good.clone()], &[f.ca_key], "museum-01", now()).unwrap();

        let cases: Vec<(&str, Cert, &str, &str)> = vec![
            (
                "expired an hour ago",
                node_cert(&f, "museum-01", -7200, -3600),
                "museum-01",
                "expired",
            ),
            (
                "not valid until tomorrow",
                node_cert(&f, "museum-01", 3600, 7200),
                "museum-01",
                "not valid yet",
            ),
            (
                "issued for another node",
                node_cert(&f, "museum-02", -60, 3600),
                "museum-01",
                "is for",
            ),
        ];
        for (what, cert, host, expect) in cases {
            let e = verify(&[cert], &[f.ca_key], host, now())
                .expect_err(&format!("{} was accepted", what));
            assert!(e.contains(expect), "{}: unhelpful refusal {:?}", what, e);
        }

        // A certificate from a CA nobody here knows.
        let stranger = fleet();
        let e = verify(&[node_cert(&stranger, "museum-01", -60, 3600)], &[f.ca_key], "museum-01", now())
            .unwrap_err();
        assert!(e.contains("does not know"), "{}", e);

        // And no CA at all, which is a configuration mistake rather than an
        // attack and still must not be an accidental success.
        let e = verify(&[good], &[], "museum-01", now()).unwrap_err();
        assert!(e.contains("no certificate authority"), "{}", e);
    }

    #[test]
    fn a_node_cannot_sign_its_own_certificate() {
        // THE ATTACK THIS FILE EXISTS FOR. A machine with a key can always
        // make a certificate saying it is museum-01; what it cannot do is get
        // the fleet CA to sign one.
        let f = fleet();
        let n = now() as i64;
        let der = make(
            &Spec {
                subject: "museum-01",
                issuer: "museum-01",
                names: &["museum-01"],
                not_before: (n - 60) as u64,
                not_after: (n + 3600) as u64,
                is_ca: true,
            },
            &f.node_key,
            &f.node_seed,
        );
        let self_signed = Cert::parse(&der).unwrap();
        assert!(self_signed.signed_by(&f.node_key), "it did sign itself");
        let e = verify(&[self_signed], &[f.ca_key], "museum-01", now()).unwrap_err();
        assert!(e.contains("does not know"), "{}", e);
    }

    #[test]
    fn a_chain_holds_together_or_it_does_not() {
        let f = fleet();
        let n = now() as i64;
        let (mid_seed, mid_key) = crypto::ed25519_keypair().unwrap();
        let mid_der = make(
            &Spec {
                subject: "copal fleet intermediate",
                issuer: "copal fleet CA",
                names: &[],
                not_before: (n - 60) as u64,
                not_after: (n + 3600) as u64,
                is_ca: true,
            },
            &mid_key,
            &f.ca_seed,
        );
        let mid = Cert::parse(&mid_der).unwrap();
        assert!(mid.is_ca, "the intermediate did not say it was a CA");

        let leaf_der = make(
            &Spec {
                subject: "museum-01",
                issuer: "copal fleet intermediate",
                names: &["museum-01"],
                not_before: (n - 60) as u64,
                not_after: (n + 3600) as u64,
                is_ca: false,
            },
            &f.node_key,
            &mid_seed,
        );
        let leaf = Cert::parse(&leaf_der).unwrap();
        verify(&[leaf.clone(), mid.clone()], &[f.ca_key], "museum-01", now()).unwrap();

        // The leaf alone is not enough, because the fleet does not know the
        // intermediate's key.
        assert!(verify(&[leaf.clone()], &[f.ca_key], "museum-01", now()).is_err());

        // An intermediate that is not a CA cannot carry a chain, even with
        // every signature in place.
        let not_ca_der = make(
            &Spec {
                subject: "copal fleet intermediate",
                issuer: "copal fleet CA",
                names: &[],
                not_before: (n - 60) as u64,
                not_after: (n + 3600) as u64,
                is_ca: false,
            },
            &mid_key,
            &f.ca_seed,
        );
        let not_ca = Cert::parse(&not_ca_der).unwrap();
        let e = verify(&[leaf, not_ca], &[f.ca_key], "museum-01", now()).unwrap_err();
        assert!(e.contains("not a certificate authority"), "{}", e);
    }

    #[test]
    fn a_wildcard_matches_nothing() {
        // `*.museum.local` would match a node nobody asked for. The fleet
        // issues one certificate per node with that node's name in it.
        let f = fleet();
        let c = node_cert(&f, "*.museum.local", -60, 3600);
        assert!(!c.matches("museum-01.museum.local"));
        assert!(!c.matches("anything.museum.local"));
        // It matches itself, literally, which is harmless and is what
        // "no wildcards" means rather than "reject the character".
        assert!(c.matches("*.museum.local"));
    }

    #[test]
    fn a_name_matches_the_way_dns_does_and_no_further() {
        let f = fleet();
        let c = node_cert(&f, "Museum-01.local", -60, 3600);
        assert!(c.matches("museum-01.local"), "case should not matter");
        assert!(c.matches("MUSEUM-01.LOCAL"));
        // A trailing dot is the same name.
        assert!(c.matches("museum-01.local."));
        assert!(!c.matches("museum-01"));
        assert!(!c.matches("museum-01.local.evil.example"));
    }

    #[test]
    fn a_certificate_that_was_edited_after_signing_does_not_verify() {
        let f = fleet();
        let n = now() as i64;
        let der = make(
            &Spec {
                subject: "museum-01",
                issuer: "copal fleet CA",
                names: &["museum-01"],
                not_before: (n - 60) as u64,
                not_after: (n + 3600) as u64,
                is_ca: false,
            },
            &f.node_key,
            &f.ca_seed,
        );
        let at = der
            .windows(9)
            .position(|w| w == b"museum-01")
            .expect("the name is in there");
        let mut edited = der.clone();
        edited[at + 8] = b'2';
        let c = Cert::parse(&edited).unwrap();
        // The first occurrence is the subject's common name; the SAN still
        // says museum-01, which is itself worth knowing -- a certificate with
        // two names in it can be edited in one of them.
        assert_eq!(c.subject, "museum-02", "the edit did not take");
        assert!(!c.signed_by(&f.ca_key), "an edited certificate still verified");
    }

    #[test]
    fn the_dates_are_read_as_the_calendar_means_them() {
        // Both encodings, and a leap day, because February is where date
        // arithmetic written by hand goes wrong.
        assert_eq!(time(0x17, b"700101000000Z").unwrap(), 0);
        assert_eq!(time(0x18, b"19700101000000Z").unwrap(), 0);
        assert_eq!(time(0x17, b"700102000000Z").unwrap(), 86400);
        assert_eq!(time(0x18, b"20240229120000Z").unwrap(), 1_709_208_000);
        assert_eq!(time(0x17, b"491231235959Z").unwrap(), 2_524_607_999);
        // 50 and above is the twentieth century, by RFC 5280 -- and anything
        // before the epoch clamps to zero rather than wrapping into the far
        // future, which is what `as u64` on a negative number does and how a
        // 1950 notBefore becomes a certificate that is never valid yet.
        assert_eq!(time(0x17, b"500101000000Z").unwrap(), 0);
        assert_eq!(time(0x18, b"19600101000000Z").unwrap(), 0);
        assert!(time(0x17, b"701301000000Z").is_err(), "month thirteen was read");
        assert!(time(0x17, b"nonsense").is_err());
    }

    #[test]
    fn anything_that_is_not_an_ed25519_certificate_is_refused() {
        assert!(Cert::parse(b"").is_err());
        assert!(Cert::parse(&[0x30, 0x03, 0x02, 0x01, 0x01]).is_err());
        // A truncated certificate must be a sentence rather than a panic: this
        // is the first thing a hostile server sends.
        let f = fleet();
        let der = node_cert(&f, "museum-01", -60, 3600).der;
        for cut in [1usize, 5, 20, der.len() / 2, der.len() - 1] {
            assert!(Cert::parse(&der[..cut]).is_err(), "a certificate cut at {} parsed", cut);
        }
    }

    #[test]
    fn pem_files_are_read_the_way_the_tools_write_them() {
        let f = fleet();
        let one = node_cert(&f, "museum-01", -60, 3600).der;
        let two = node_cert(&f, "museum-02", -60, 3600).der;
        let pem = |d: &[u8]| {
            let b = crypto::b64_encode(d);
            let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
            for chunk in b.as_bytes().chunks(64) {
                out.push_str(std::str::from_utf8(chunk).unwrap());
                out.push('\n');
            }
            out.push_str("-----END CERTIFICATE-----\n");
            out
        };
        let text = format!("# a comment tools put here\n{}{}", pem(&one), pem(&two));
        let chain = from_pem(&text).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].subject, "museum-01");
        assert_eq!(chain[1].subject, "museum-02");
        assert!(from_pem("nothing here").is_err());
    }
}

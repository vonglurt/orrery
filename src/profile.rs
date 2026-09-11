//! The cryptographic profile: one whitelist, versioned, shared by both ends.
//!
//! THE ASK THIS ANSWERS. A fleet node must not be reachable by a standard
//! client. Not "is hard to reach", not "warns" -- a stock `ssh`, `mstsc`,
//! FreeRDP, Remmina or KRDC must fail to negotiate, at the handshake, before
//! authentication is ever offered. And the set of things that ARE accepted must
//! be one list in one place, because a whitelist written twice is two lists
//! that drift and a node whose sshd is more permissive than the console's
//! client is a node the console's narrowness was not protecting.
//!
//! So this module is that list, and it is the ONLY copy. Three things read it:
//!
//!   * `ssh.rs` and `tls.rs` build their offers from it -- the console cannot
//!     negotiate something the profile does not name, because it has nothing
//!     else to offer;
//!   * `sshd_config()` renders the lines `copal-prep.sh` writes into the
//!     node's `Match` block, so the node refuses exactly what the console
//!     cannot speak;
//!   * `fleet-control.md` §2's `profile` reading travels in the node's beacon,
//!     so the wall can show a node whose profile has drifted.
//!
//! THE VERSION IS THE POINT, NOT DECORATION. "Only allow whitelisted versions"
//! means a node states which profile it enforces and the console refuses to
//! connect to one that is older than the minimum it accepts. A rotation is
//! therefore a number that goes up, a render of the sshd lines, and a rebuild
//! -- not an archaeology exercise across 1.6 MB of shell.
//!
//! WHAT THIS IS NOT. It is not a second authentication system. The fleet's gate
//! is the certificate authority of `fleet-plan.md` §5, and everything here
//! narrows what may be spoken AFTER that gate decides. Narrowing is not a
//! substitute for the gate and is not described as one.

/// The profile this build speaks and enforces.
///
/// Bump it when the lists below change in a way that makes an older peer
/// unacceptable. A node announcing less than `MIN_ACCEPTED` is refused.
pub const CURRENT: u32 = 1;

/// The oldest profile this console will talk to.
///
/// Equal to `CURRENT` today, and they are separate constants so that a fleet
/// mid-rotation -- six cards reflashed, two not yet -- can be driven for an
/// afternoon by lowering one number rather than by widening a cipher list.
/// Lowering this is a deliberate, visible, temporary act; widening a list is
/// none of those things.
pub const MIN_ACCEPTED: u32 = 1;

/// Everything one profile permits.
#[derive(Debug, Clone, Copy)]
pub struct Profile {
    pub version: u32,

    // -- SSH ---------------------------------------------------------------
    /// One kex. Curve25519 has no parameters to get wrong and no small
    /// subgroup to check for, which is the reason it is the only one here.
    pub ssh_kex: &'static [&'static str],
    /// CERTIFICATES ONLY. A bare `ssh-ed25519` host key means trust on first
    /// use, and trust on first use is exactly the decision this fleet already
    /// took away from the operator by having a CA.
    pub ssh_host_key: &'static [&'static str],
    /// AEAD only, so there is no separate MAC to negotiate and no
    /// encrypt-then-MAC-versus-MAC-then-encrypt question to answer.
    pub ssh_cipher: &'static [&'static str],
    /// Deliberately empty. Both ciphers carry their own tag.
    pub ssh_mac: &'static [&'static str],
    /// What a client may authenticate WITH. Certificates only, again.
    pub ssh_pubkey_accepted: &'static [&'static str],

    // -- TLS 1.3, under RDP ------------------------------------------------
    /// The only version. `0x0304` and nothing else means no downgrade dance.
    pub tls_versions: &'static [u16],
    pub tls_suites: &'static [&'static str],
    pub tls_groups: &'static [&'static str],
    pub tls_sig_algs: &'static [&'static str],

    // -- RDP ---------------------------------------------------------------
    /// The X.224 negotiation request's `requestedProtocols`.
    ///
    /// `PROTOCOL_SSL` (1) alone. Not `PROTOCOL_RDP` (0), which is RC4 and a
    /// 512-bit RSA key and is broken; not `PROTOCOL_HYBRID` (2), which is
    /// CredSSP and therefore NTLM -- a credential-forwarding design whose
    /// purpose is to carry a secret to a machine before that machine has
    /// proved anything.
    pub rdp_protocol: u32,
    /// Whether the node demands a client certificate from the fleet CA.
    ///
    /// THIS IS THE ONE THAT STOPS A STANDARD CLIENT. Everything else on this
    /// list narrows what may be spoken; this decides who may speak at all.
    /// mstsc has no fleet certificate and cannot be given one, so its TLS
    /// handshake ends at `certificate_required` before RDP begins.
    pub rdp_client_cert: bool,
}

pub const P1: Profile = Profile {
    version: 1,

    ssh_kex: &["curve25519-sha256"],
    ssh_host_key: &["ssh-ed25519-cert-v01@openssh.com"],
    ssh_cipher: &["chacha20-poly1305@openssh.com", "aes256-gcm@openssh.com"],
    ssh_mac: &[],
    ssh_pubkey_accepted: &["ssh-ed25519-cert-v01@openssh.com"],

    tls_versions: &[0x0304],
    tls_suites: &["TLS_CHACHA20_POLY1305_SHA256", "TLS_AES_256_GCM_SHA384"],
    tls_groups: &["x25519"],
    tls_sig_algs: &["ed25519"],

    rdp_protocol: 1,
    rdp_client_cert: true,
};

pub fn current() -> &'static Profile {
    &P1
}

/// Is a peer announcing profile `v` one this console will talk to?
pub fn accepts(v: u32) -> bool {
    v >= MIN_ACCEPTED && v <= CURRENT
}

/// Why a peer was refused, as the sentence the operator sees.
pub fn refusal(v: u32) -> Option<String> {
    if accepts(v) {
        return None;
    }
    Some(if v < MIN_ACCEPTED {
        format!(
            "this node enforces crypto profile {}, and the console accepts {} or newer. \
             Reflash it, or lower the minimum for this session and say why.",
            v, MIN_ACCEPTED
        )
    } else {
        format!(
            "this node enforces crypto profile {}, which is newer than this console's {}. \
             Update the console rather than widening the node.",
            v, CURRENT
        )
    })
}

/// The lines `copal-prep.sh` writes into the node's sshd policy.
///
/// RENDERED, NOT RETYPED. `make lint` compares this output against the block in
/// `copal-prep.sh` and fails on drift, the same way it already compares the
/// embedded copy of `radbeeper` and of `copal_nkeys.py` against their sources.
/// A whitelist that exists in a Rust file and again in a shell heredoc is two
/// whitelists.
pub fn sshd_config(p: &Profile) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Copal crypto profile {}. Rendered by orrery's src/profile.rs --\n\
         # do not edit here; edit there and re-run `make sync-profile`.\n",
        p.version
    ));
    out.push_str(&format!("KexAlgorithms {}\n", p.ssh_kex.join(",")));
    out.push_str(&format!("HostKeyAlgorithms {}\n", p.ssh_host_key.join(",")));
    out.push_str(&format!("CASignatureAlgorithms {}\n", "ssh-ed25519"));
    out.push_str(&format!("Ciphers {}\n", p.ssh_cipher.join(",")));
    out.push_str(&format!(
        "PubkeyAcceptedAlgorithms {}\n",
        p.ssh_pubkey_accepted.join(",")
    ));
    // PUBLIC KEYS AND NOTHING ELSE. `AuthenticationMethods publickey` is the
    // one that makes it true rather than merely default: with it, sshd will
    // not complete a login by any other path even if a later line, a package
    // upgrade or an edit turns one of them back on.
    out.push_str("AuthenticationMethods publickey\n");
    out.push_str("PasswordAuthentication no\n");
    out.push_str("KbdInteractiveAuthentication no\n");
    out.push_str("ChallengeResponseAuthentication no\n");
    out.push_str("GSSAPIAuthentication no\n");
    out.push_str("HostbasedAuthentication no\n");
    out.push_str("PermitEmptyPasswords no\n");
    out.push_str("PermitRootLogin no\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The list, read as a policy rather than as data.
    ///
    /// This test is the review. Anything that appears in the profile and is
    /// named here fails the build, so widening the whitelist to something weak
    /// is not something that can happen quietly during a debugging session.
    #[test]
    fn nothing_weak_is_on_any_list() {
        let p = current();
        let every: Vec<String> = p
            .ssh_kex
            .iter()
            .chain(p.ssh_host_key)
            .chain(p.ssh_cipher)
            .chain(p.ssh_mac)
            .chain(p.ssh_pubkey_accepted)
            .chain(p.tls_suites)
            .chain(p.tls_groups)
            .chain(p.tls_sig_algs)
            .map(|s| s.to_ascii_lowercase())
            .collect();

        for banned in [
            "sha1", "-sha1", "md5", "rc4", "3des", "des", "cbc", "arcfour", "blowfish",
            "cast128", "umac-64", "diffie-hellman-group1", "diffie-hellman-group14",
            "group-exchange-sha1", "ssh-rsa", "ssh-dss", "ecdsa", "nistp", "null",
            "export", "anon", "rc2", "idea", "seed", "camellia",
        ] {
            for item in &every {
                assert!(
                    !item.contains(banned),
                    "profile {} offers {:?}, which contains the banned token {:?}",
                    p.version,
                    item,
                    banned
                );
            }
        }
    }

    #[test]
    fn every_ssh_cipher_carries_its_own_tag_so_there_is_no_mac_to_negotiate() {
        let p = current();
        assert!(p.ssh_mac.is_empty(), "a separate MAC crept onto the list");
        for c in p.ssh_cipher {
            assert!(
                c.contains("gcm") || c.contains("poly1305"),
                "{} is not an AEAD, so it would need a MAC",
                c
            );
        }
    }

    #[test]
    fn only_certificates_are_accepted_at_either_end() {
        // A bare key means trust on first use, and the whole point of the
        // fleet having a CA is that nobody is ever asked to decide.
        let p = current();
        for k in p.ssh_host_key {
            assert!(k.contains("cert-v01"), "{} is a bare host key", k);
        }
        for k in p.ssh_pubkey_accepted {
            assert!(k.contains("cert-v01"), "{} is a bare user key", k);
        }
    }

    #[test]
    fn tls_is_one_three_and_only_one_three() {
        let p = current();
        assert_eq!(p.tls_versions, &[0x0304], "a second TLS version is a downgrade dance");
    }

    #[test]
    fn rdp_asks_for_tls_and_refuses_both_of_the_alternatives() {
        let p = current();
        // 1 is PROTOCOL_SSL. 0 would be RDP's own RC4 security layer and 2
        // would be CredSSP, which is NTLM, which is a credential-forwarding
        // design. Neither is negotiable because neither is offered.
        assert_eq!(p.rdp_protocol, 1);
        assert_eq!(p.rdp_protocol & 2, 0, "CredSSP was requested");
    }

    #[test]
    fn a_standard_rdp_client_cannot_reach_a_node() {
        // The property, stated as a test: the node demands a client
        // certificate from the fleet CA, and mstsc has none and cannot be
        // given one. Everything else on the profile narrows what may be
        // SPOKEN; this decides who may SPEAK.
        assert!(
            current().rdp_client_cert,
            "without a client certificate the node is reachable by any RDP client"
        );
    }

    #[test]
    fn the_sshd_block_forbids_every_path_that_is_not_a_public_key() {
        let cfg = sshd_config(current());
        // The line that makes it true rather than merely default.
        assert!(cfg.contains("AuthenticationMethods publickey"));
        for off in [
            "PasswordAuthentication no",
            "KbdInteractiveAuthentication no",
            "ChallengeResponseAuthentication no",
            "GSSAPIAuthentication no",
            "HostbasedAuthentication no",
            "PermitEmptyPasswords no",
            "PermitRootLogin no",
        ] {
            assert!(cfg.contains(off), "the sshd block does not say {:?}", off);
        }
        // And it names the profile it came from, so a node can be read.
        assert!(cfg.contains("crypto profile 1"));
    }

    #[test]
    fn the_sshd_block_names_exactly_the_algorithms_the_console_can_speak() {
        let p = current();
        let cfg = sshd_config(p);
        assert!(cfg.contains(&format!("KexAlgorithms {}", p.ssh_kex.join(","))));
        assert!(cfg.contains(&format!("Ciphers {}", p.ssh_cipher.join(","))));
        // The node must not be more permissive than the client. If it names
        // something the profile does not, the console's narrowness was
        // protecting nothing.
        for line in cfg.lines() {
            if let Some(rest) = line.strip_prefix("Ciphers ") {
                for named in rest.split(',') {
                    assert!(
                        p.ssh_cipher.contains(&named),
                        "the node would accept {:?}, which the console cannot speak",
                        named
                    );
                }
            }
        }
    }

    #[test]
    fn a_peer_on_an_older_profile_is_refused_with_a_sentence() {
        assert!(accepts(CURRENT));
        assert!(!accepts(0));
        assert!(!accepts(CURRENT + 1));
        assert!(refusal(CURRENT).is_none());

        let old = refusal(0).unwrap();
        assert!(old.contains("Reflash"), "the refusal does not say what to do: {}", old);
        let new = refusal(CURRENT + 1).unwrap();
        assert!(
            new.contains("Update the console"),
            "a newer node should move the console, not widen the node: {}",
            new
        );
    }

    #[test]
    fn the_minimum_is_never_quietly_below_the_current_profile() {
        // Lowering MIN_ACCEPTED is a deliberate, visible, temporary act for a
        // fleet mid-rotation. This does not forbid it -- it makes the diff
        // land here, where it is read, rather than inside a cipher list.
        assert!(
            MIN_ACCEPTED == CURRENT,
            "MIN_ACCEPTED is {} against a current profile of {} -- \
             if this is a rotation in progress, say so here and in the commit",
            MIN_ACCEPTED,
            CURRENT
        );
    }
}

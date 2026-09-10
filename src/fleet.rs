//! The read model: one subprocess, one document, no cache.
//!
//! `copal fleet state --json` IS the answer, so this holds nothing between
//! requests. A cache would be a second place the fleet's state lives and a
//! thing that goes stale; the CLI already owns the question.
//!
//! The document is forwarded to the browser as the CLI wrote it. Parsing it
//! only to write it out again would add a way for the two to disagree about
//! what a node is -- so the only thing added on the way past is the header's
//! two sentences, spliced in as three keys.

use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::json;

/// A verb may take three minutes; the read model may take forty-five seconds.
/// A scene applied over ssh to eight boards is slow and that is not a fault.
pub const VERB_TIMEOUT: Duration = Duration::from_secs(180);
pub const STATE_TIMEOUT: Duration = Duration::from_secs(45);
/// Long enough not to spin a core, short enough that a fast verb still feels
/// immediate to the operator who pressed it.
const POLL: Duration = Duration::from_millis(20);

pub struct Fleet {
    pub cmd: Vec<String>,
    pub fleet: String,
    pub demo: bool,
    /// Verbs are serialised. Two operators pressing Power at once should be
    /// two commands in a row, not two `copal fleet` processes racing over the
    /// same node.
    lock: Mutex<()>,
}

impl Fleet {
    pub fn new(cmd: Vec<String>, fleet: String, demo: bool) -> Self {
        Fleet { cmd, fleet, demo, lock: Mutex::new(()) }
    }

    /// `copal fleet <verb> --fleet NAME [the verb's own args]`.
    ///
    /// THE POSITION IS NOT COSMETIC, and it is wrong in two different ways if
    /// guessed. `copal-fleet.sh` dispatches on its first word, so an option
    /// before the verb is read AS the verb -- "no fleet verb called
    /// '--fleet'". And `cmd_run` ends its option loop with `_verb="$*"`,
    /// swallowing everything after the verb word so that it can pass arbitrary
    /// arguments through to the node -- so an option after the verb's own
    /// arguments is silently eaten and the CLI reports "no fleet named".
    ///
    /// The one position that works everywhere is immediately after the verb
    /// word and before the verb's arguments. Neither mistake was reachable
    /// until this was pointed at the real CLI.
    fn argv(&self, tail: &[String]) -> Vec<String> {
        let mut v = self.cmd.clone();
        if tail.is_empty() {
            return v;
        }
        v.push(tail[0].clone());
        if !self.fleet.is_empty() {
            v.push("--fleet".into());
            v.push(self.fleet.clone());
        }
        v.extend_from_slice(&tail[1..]);
        v
    }

    #[cfg(test)]
    pub fn argv_for_test(&self, tail: &[String]) -> Vec<String> {
        self.argv(tail)
    }

    /// Run the CLI with a deadline.
    ///
    /// `Command::output()` waits forever, and the prototype this replaces had
    /// a timeout for a reason: a verb that hangs on a node with a wedged NFS
    /// mount must not take the console with it. std has no timed wait, so the
    /// child is spawned, its pipes are drained by threads (a child that fills
    /// a pipe buffer blocks, and then `try_wait` never becomes ready), and the
    /// wait is a poll against a deadline. `TimedOut` is returned as an error
    /// kind the callers already distinguish.
    fn run_within(&self, tail: &[String], budget: Duration) -> io::Result<(i32, String, String)> {
        let argv = self.argv(tail);
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let mut out_pipe = child.stdout.take();
        let mut err_pipe = child.stderr.take();
        let out_reader = thread::spawn(move || {
            let mut buf = String::new();
            if let Some(p) = out_pipe.as_mut() {
                let _ = p.read_to_string(&mut buf);
            }
            buf
        });
        let err_reader = thread::spawn(move || {
            let mut buf = String::new();
            if let Some(p) = err_pipe.as_mut() {
                let _ = p.read_to_string(&mut buf);
            }
            buf
        });

        let deadline = Instant::now() + budget;
        let status = loop {
            match child.try_wait()? {
                Some(s) => break s,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out after {}s", budget.as_secs()),
                    ));
                }
                None => thread::sleep(POLL),
            }
        };

        let out = out_reader.join().unwrap_or_default();
        let err = err_reader.join().unwrap_or_default();
        Ok((status.code().unwrap_or(-1), out, err))
    }

    /// The document, as text, ready to forward.
    pub fn state(&self) -> Result<String, String> {
        if self.demo {
            return Ok(demo_doc());
        }
        match self.run_within(&["state".into(), "--json".into()], STATE_TIMEOUT) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(format!("{} is not on PATH", self.cmd[0]))
            }
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Err(format!(
                "`{} state` did not answer in {}s",
                self.cmd.join(" "),
                STATE_TIMEOUT.as_secs()
            )),
            Err(e) => Err(format!("could not run {}: {}", self.cmd.join(" "), e)),
            Ok((code, _, err)) if code != 0 => {
                let msg = err.trim();
                Err(if msg.is_empty() {
                    format!("`{}` exited {}", self.cmd.join(" "), code)
                } else {
                    truncate(msg, 400)
                })
            }
            Ok((_, out, _)) => Ok(out),
        }
    }

    /// A verb, aimed at each node in turn.
    ///
    /// The CLI selects with `--node ID`, one id at a time, so a selection of
    /// eight is eight runs. They are deliberately sequential: eight ssh
    /// sessions opened at once from a Pi is a worse morning than eight opened
    /// in a row, and the operator gets a result per node either way.
    ///
    /// A node that fails does not stop the ones after it. "Six of eight got
    /// the memo" is the normal case and the console has to be able to say so.
    pub fn verb_each(&self, plan: &crate::verbs::Plan, nodes: &[String]) -> Vec<NodeResult> {
        if !plan.per_node {
            let (code, output) = self.verb(&plan.argv);
            return vec![NodeResult { node: "fleet".to_string(), code, output }];
        }
        nodes
            .iter()
            .map(|n| {
                let argv = crate::verbs::aimed_at(plan, n);
                let (code, output) = self.verb(&argv);
                NodeResult { node: n.clone(), code, output }
            })
            .collect()
    }

    /// A verb. Returns the exit code and whatever it said, both of which the
    /// operator sees -- "I told eight machines to shut down" and "eight
    /// machines shut down" are different claims.
    pub fn verb(&self, tail: &[String]) -> (i32, String) {
        if self.demo {
            return (
                0,
                format!("demo: would have run `copal fleet {}`", tail.join(" ")),
            );
        }
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        match self.run_within(tail, VERB_TIMEOUT) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                (127, format!("{} is not on PATH", self.cmd[0]))
            }
            // "I told eight machines to shut down" and "eight machines shut
            // down" are different claims, and a timeout is the case where only
            // the first one is true. Say so rather than report a failure.
            Err(e) if e.kind() == io::ErrorKind::TimedOut => (
                124,
                format!(
                    "timed out after {}s -- it may still be running",
                    VERB_TIMEOUT.as_secs()
                ),
            ),
            Err(e) => (126, format!("could not run the verb: {}", e)),
            Ok((code, out, err)) => (
                code,
                truncate(&strip_ansi(format!("{}{}", out, err).trim()), 4000),
            ),
        }
    }
}

/// One node's answer to one verb.
pub struct NodeResult {
    pub node: String,
    pub code: i32,
    pub output: String,
}

/// Drop ANSI colour from CLI output.
///
/// `copal-fleet.sh` colours its errors for a terminal, and a browser renders
/// `ESC[31m` as literal rubbish in front of the sentence the operator needs to
/// read. The text is the message; the colour was for somewhere else.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            // A CSI sequence ends at the first byte in @-~.
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// Splice the header's two sentences into the document.
///
/// The degradation run of 2026-09-08 found that a fallback which is labelled
/// but not explained is still a quiet fallback: `bus off: timed out` reports a
/// missing component without telling the operator that the tiles in front of
/// them are four-minute-old beacons. So both halves always travel together.
pub fn annotate(doc_text: &str) -> String {
    let (live, why) = match json::parse(doc_text) {
        Ok(v) => {
            let bus = v.get("bus");
            let live = matches!(
                bus.and_then(|b| b.get("reachable")),
                Some(json::Value::Bool(true))
            );
            let why = bus
                .and_then(|b| b.get("why"))
                .and_then(|w| w.as_str())
                .unwrap_or("the bus is unreachable")
                .to_string();
            (live, why)
        }
        // Unreadable JSON is itself a picture the operator should get, rather
        // than a spinner. The page draws the error.
        Err(e) => {
            return format!(
                "{{\"error\":{}}}",
                json::Value::quote(&format!("unreadable JSON from the CLI: {}", e))
            )
        }
    };
    let head = if live {
        "\"_live\":true,\"_picture\":\"live\",\"_why\":\"\",".to_string()
    } else {
        format!(
            "\"_live\":false,\"_picture\":\"polled, not live\",\"_why\":{},",
            json::Value::quote(&why)
        )
    };
    match doc_text.find('{') {
        Some(i) => {
            let rest = doc_text[i + 1..].trim_start();
            if rest.starts_with('}') {
                format!("{{{}}}", head.trim_end_matches(','))
            } else {
                format!("{{{}{}", head, rest)
            }
        }
        None => doc_text.to_string(),
    }
}

/// The lab report's museum, so the wall can be looked at without a fleet.
///
/// Deliberately not eight healthy nodes. The frames worth designing against
/// are the ones with a machine that never announced, one whose agent has gone
/// quiet, and a stranger on the network.
pub fn demo_doc() -> String {
    let mut nodes = Vec::new();
    for i in 1..=8 {
        let (tags, extra): (&str, String) = match i {
            1 | 2 => ("[\"wall\"]", String::new()),
            3 => (
                "[\"sdr\"]",
                "\"job\":\"sdr-source\",\"temp_c\":51,".into(),
            ),
            5 => ("[\"north\"]", String::new()),
            6 => (
                "[\"north\"]",
                "\"role\":\"warden\",\"scene\":\"wake\",\"temp_c\":39,".into(),
            ),
            7 => (
                "[\"north\"]",
                "\"announced\":false,\"on_bus\":false,\"status\":\"down\",\
                 \"scene\":null,\"temp_c\":null,\"agent\":null,\"job\":null,\
                 \"last_seen\":\"08:12\","
                    .into(),
            ),
            8 => (
                "[]",
                "\"scene\":\"rest\",\"on_bus\":false,\"agent\":\"silent\",\"temp_c\":48,".into(),
            ),
            _ => ("[]", String::new()),
        };
        // The per-node overrides are emitted LAST on purpose: a later key wins,
        // so putting them first silently restored every default behind them --
        // which is how the fixture briefly claimed museum-07 had announced.
        nodes.push(format!(
            "{{\"id\":\"museum-{i:02}\",\"address\":\"10.0.0.{addr}\",\
             \"role\":\"node\",\"status\":\"up\",\"scene\":\"show\",\"scene_min\":14,\
             \"build\":\"2026-09-04.3\",\"alpine\":\"3.24.1\",\"arch\":\"aarch64\",\
             \"temp_c\":{temp},\"agent\":\"up\",\"heard_s\":{heard},\"uptime_min\":{up},\
             \"announced\":true,\"on_bus\":true,\"declared\":true,\"tags\":{tags},\
             \"ram_mb\":1024,\"job\":\"smallpt\",\"cert_days\":87,\"apk_pending\":3,\
             \"thumb\":null,\"vnc\":null,{extra}\"_fixture\":true}}",
            extra = extra,
            i = i,
            addr = 10 + i,
            temp = 44 + (i % 5),
            heard = i % 4,
            up = 300 + i,
            tags = tags,
        ));
    }
    format!(
        "{{\"fleet\":\"museum\",\"at\":\"2026-09-10T09:58:00\",\"warden\":\"museum-06\",\
         \"bus\":{{\"reachable\":true,\"why\":null}},\
         \"counts\":{{\"declared\":8,\"announced\":7,\"on_bus\":6,\"missing\":1}},\
         \"scenes\":{{\"show\":[\"museum-01\",\"museum-02\",\"museum-03\",\"museum-04\",\
         \"museum-05\"],\"wake\":[\"museum-06\"],\"rest\":[\"museum-08\"]}},\
         \"strangers\":[{{\"id\":\"epson-XY10\",\"address\":\"10.0.0.44\",\"fleet\":null}}],\
         \"nodes\":[{}]}}",
        nodes.join(",")
    )
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verb_comes_before_the_fleet_flag() {
        let f = Fleet::new(
            vec!["copal".into(), "fleet".into()],
            "museum".into(),
            false,
        );
        assert_eq!(
            f.argv_for_test(&["state".into(), "--json".into()]),
            ["copal", "fleet", "state", "--fleet", "museum", "--json"]
        );
        // `run` is the case that proves the position: its own arguments must
        // stay together after the options, or cmd_run swallows them.
        assert_eq!(
            f.argv_for_test(&[
                "run".into(),
                "snapshot".into(),
                "restore".into(),
                "--node".into(),
                "museum-01".into()
            ]),
            [
                "copal", "fleet", "run", "--fleet", "museum", "snapshot",
                "restore", "--node", "museum-01"
            ]
        );
        // With no fleet named, nothing is appended at all.
        let g = Fleet::new(vec!["copal".into(), "fleet".into()], String::new(), false);
        assert_eq!(
            g.argv_for_test(&["state".into()]),
            ["copal", "fleet", "state"]
        );
    }

    #[test]
    fn colour_meant_for_a_terminal_does_not_reach_the_browser() {
        assert_eq!(strip_ansi("\u{1b}[31merror:\u{1b}[0m no fleet named"), "error: no fleet named");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[0m"), "ok");
    }

    #[test]
    fn the_fixture_is_valid_json() {
        let v = json::parse(&demo_doc()).expect("fixture does not parse");
        assert_eq!(v.get("fleet").unwrap().as_str(), Some("museum"));
        assert_eq!(v.get("nodes").unwrap().as_array().unwrap().len(), 8);
    }

    #[test]
    fn the_fixture_keeps_its_awkward_nodes() {
        let v = json::parse(&demo_doc()).unwrap();
        let nodes = v.get("nodes").unwrap().as_array().unwrap();
        let absent = nodes
            .iter()
            .filter(|n| n.get("announced") == Some(&json::Value::Bool(false)))
            .count();
        let quiet = nodes
            .iter()
            .filter(|n| n.get("agent").and_then(|a| a.as_str()) == Some("silent"))
            .count();
        assert_eq!(absent, 1, "no absent node in the fixture");
        assert_eq!(quiet, 1, "no quiet agent in the fixture");
        // A node that never announced must not carry a temperature: "not
        // reported" and "45" are different claims.
        let seven = nodes
            .iter()
            .find(|n| n.get("id").and_then(|i| i.as_str()) == Some("museum-07"))
            .unwrap();
        assert_eq!(seven.get("temp_c"), Some(&json::Value::Null));
    }

    #[test]
    fn a_live_picture_says_live() {
        let out = annotate(r#"{"bus":{"reachable":true,"why":null},"nodes":[]}"#);
        let v = json::parse(&out).unwrap();
        assert_eq!(v.get("_live"), Some(&json::Value::Bool(true)));
        assert_eq!(v.get("_picture").unwrap().as_str(), Some("live"));
        assert!(v.get("nodes").is_some(), "the document was lost");
    }

    #[test]
    fn a_bus_off_picture_is_labelled_and_explained() {
        let out = annotate(r#"{"bus":{"reachable":false,"why":"timed out"},"nodes":[]}"#);
        let v = json::parse(&out).unwrap();
        assert_eq!(
            v.get("_picture").unwrap().as_str(),
            Some("polled, not live")
        );
        assert_eq!(v.get("_why").unwrap().as_str(), Some("timed out"));
    }

    #[test]
    fn annotating_the_fixture_keeps_every_node() {
        let v = json::parse(&annotate(&demo_doc())).unwrap();
        assert_eq!(v.get("nodes").unwrap().as_array().unwrap().len(), 8);
        assert_eq!(v.get("warden").unwrap().as_str(), Some("museum-06"));
    }

    #[test]
    fn unreadable_json_becomes_a_picture_of_the_failure() {
        let v = json::parse(&annotate("not json at all")).unwrap();
        assert!(v.get("error").is_some());
    }

    #[test]
    fn an_empty_document_survives_annotation() {
        let v = json::parse(&annotate("{}")).unwrap();
        assert_eq!(v.get("_live"), Some(&json::Value::Bool(false)));
    }
}

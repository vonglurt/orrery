//! The allow-list, which is the security boundary.
//!
//! Invariant 4 says automation gets a forced command and not a shell. The same
//! discipline applies one layer up: nothing from a request is ever concatenated
//! into a command line. A verb must be a member of `WRITE_VERBS`, an argument
//! must pass the shape its verb declares, and a node id must appear in the
//! document `copal fleet state` just returned.
//!
//! Rust makes one part of this easier than the prototype did -- `Command` takes
//! argv directly and never goes near a shell -- and one part no easier at all,
//! which is deciding what a legal argument is. That part is here.

use crate::json::Value;

/// What kind of argument a verb takes, if any.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arg {
    None,
    /// A scene or job name: a git-checkout filename, not free text.
    Name,
    /// on / off / reboot, and nothing else.
    Power,
    /// A banner for a visitor to read.
    Text,
}

pub struct Verb {
    pub name: &'static str,
    /// argv with `{arg}` and `{nodes}` as the two holes.
    pub argv: &'static [&'static str],
    pub arg: Arg,
    /// Control and Exchange are refused on a selection. §IV-C of the lab
    /// report is emphatic that this is a decision rather than a gap:
    /// broadcasting keystrokes to eight machines produces divergent state
    /// nobody can see, and the operation actually wanted is Scene or Run.
    pub single_only: bool,
}

pub const WRITE_VERBS: &[Verb] = &[
    Verb { name: "scene",    argv: &["scene", "{arg}", "--nodes", "{nodes}"],       arg: Arg::Name,  single_only: false },
    Verb { name: "run",      argv: &["run", "{arg}", "--nodes", "{nodes}"],         arg: Arg::Name,  single_only: false },
    Verb { name: "power",    argv: &["power", "{arg}", "--nodes", "{nodes}"],       arg: Arg::Power, single_only: false },
    Verb { name: "snapshot", argv: &["snapshot", "restore", "--nodes", "{nodes}"],  arg: Arg::None,  single_only: false },
    Verb { name: "message",  argv: &["notify", "--text", "{arg}", "--nodes", "{nodes}"], arg: Arg::Text, single_only: false },
];

pub fn lookup(name: &str) -> Option<&'static Verb> {
    WRITE_VERBS.iter().find(|v| v.name == name)
}

/// A request that will not be run, carrying the sentence the operator sees.
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn refuse<T>(msg: impl Into<String>) -> Result<T, Refused> {
    Err(Refused(msg.into()))
}

/// Every node id the read model currently knows.
pub fn known_nodes(doc: &Value) -> Vec<String> {
    doc.get("nodes")
        .and_then(|n| n.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n.get("id").and_then(|i| i.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A name in a POST body is a candidate; a name in the state document has been
/// through the certificate check. Only the second kind becomes argv.
pub fn validate_nodes(want: &[String], doc: &Value) -> Result<Vec<String>, Refused> {
    let known = known_nodes(doc);
    if want.is_empty() {
        return refuse("no nodes selected");
    }
    let mut out = Vec::with_capacity(want.len());
    for n in want {
        if !known.iter().any(|k| k == n) {
            return refuse(format!("no such node in this fleet: {}", n));
        }
        out.push(n.clone());
    }
    Ok(out)
}

fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub fn validate_arg(kind: Arg, value: Option<&str>) -> Result<Option<String>, Refused> {
    match kind {
        Arg::None => Ok(None),
        Arg::Power => match value {
            Some(v @ ("on" | "off" | "reboot")) => Ok(Some(v.to_string())),
            _ => refuse("power takes on, off or reboot"),
        },
        Arg::Text => match value.map(str::trim) {
            Some(t) if !t.is_empty() => Ok(Some(t.chars().take(200).collect())),
            _ => refuse("a message needs text"),
        },
        Arg::Name => match value {
            Some(v) if is_name(v) => Ok(Some(v.to_string())),
            other => refuse(format!("not a usable name: {:?}", other.unwrap_or(""))),
        },
    }
}

/// Build the argv for `copal fleet`. No shell, no interpolation, no escaping
/// to get wrong -- each element is pushed whole.
pub fn build_argv(
    verb: &Verb,
    nodes: &[String],
    arg: Option<&str>,
) -> Result<Vec<String>, Refused> {
    if verb.single_only && nodes.len() != 1 {
        return refuse(format!("{} works on one node", verb.name));
    }
    let arg = validate_arg(verb.arg, arg)?;
    let joined = nodes.join(",");
    let mut out = Vec::with_capacity(verb.argv.len());
    for tok in verb.argv {
        match *tok {
            "{nodes}" => out.push(joined.clone()),
            "{arg}" => out.push(
                arg.clone()
                    .ok_or_else(|| Refused("this verb needs an argument".into()))?,
            ),
            lit => out.push(lit.to_string()),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::demo_doc;
    use crate::json;

    fn doc() -> json::Value {
        json::parse(&demo_doc()).unwrap()
    }

    #[test]
    fn the_fixture_is_the_museums_eight() {
        assert_eq!(known_nodes(&doc()).len(), 8);
    }

    #[test]
    fn takes_nodes_the_read_model_returned() {
        let ns = vec!["museum-01".to_string(), "museum-03".to_string()];
        assert_eq!(validate_nodes(&ns, &doc()).unwrap(), ns);
    }

    #[test]
    fn refuses_a_node_that_is_not_in_the_fleet() {
        for bad in [
            "../../etc/passwd",
            "museum-01; rm -rf /",
            "nope",
            "museum-01 museum-02",
            "epson-XY10", // a stranger is seen, never contacted
        ] {
            let got = validate_nodes(&[bad.to_string()], &doc());
            assert!(got.is_err(), "accepted {:?}", bad);
        }
        assert!(validate_nodes(&[], &doc()).is_err());
    }

    #[test]
    fn refuses_arguments_that_are_not_the_shape_declared() {
        for bad in ["halt", "", "off; reboot", "OFF"] {
            assert!(validate_arg(Arg::Power, Some(bad)).is_err(), "power {:?}", bad);
        }
        assert_eq!(
            validate_arg(Arg::Power, Some("off")).unwrap().as_deref(),
            Some("off")
        );
        for bad in ["../x", "a b", "$(id)", "-rf", "", &"x".repeat(80)] {
            assert!(validate_arg(Arg::Name, Some(bad)).is_err(), "name {:?}", bad);
        }
        assert!(validate_arg(Arg::Name, Some("sdr-waterfall")).is_ok());
        assert!(validate_arg(Arg::Text, Some("   ")).is_err());
        assert_eq!(
            validate_arg(Arg::Text, Some("  please stand back "))
                .unwrap()
                .as_deref(),
            Some("please stand back")
        );
    }

    #[test]
    fn builds_the_argv_the_cli_expects() {
        let v = lookup("scene").unwrap();
        let ns = vec!["museum-01".to_string(), "museum-02".to_string()];
        let argv = build_argv(v, &ns, Some("show")).unwrap();
        assert_eq!(argv, ["scene", "show", "--nodes", "museum-01,museum-02"]);
        assert!(!argv.iter().any(|a| a.contains('{')));
    }

    #[test]
    fn control_never_became_a_fanned_out_verb() {
        assert!(lookup("control").is_none());
        assert!(lookup("exchange").is_none());
        assert!(lookup("observe").is_none());
    }
}

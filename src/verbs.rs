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
    /// A sentence for a screen: letters, digits, spaces and six marks of
    /// punctuation, 200 characters at most. The node enforces the same rule.
    Banner,
}

pub struct Verb {
    pub name: &'static str,
    /// argv for `copal fleet`, with `{arg}` as the only hole. The node is NOT
    /// in here: the real CLI selects with `--node ID`, one id at a time, so
    /// the selection is applied by running this once per node.
    pub argv: &'static [&'static str],
    pub arg: Arg,
    /// False for the one verb that cannot be aimed at a node over ssh.
    pub per_node: bool,
}

/// The verbs, as `tools/copal-fleet.sh` actually implements them.
///
/// THIS TABLE WAS WRONG UNTIL IT WAS POINTED AT THE REAL CLI. It was written
/// against the shape the prototype invented -- `scene NAME --nodes a,b` -- and
/// the CLI takes `--node ID`, singular, or `--tag`. Four corrections came out
/// of running it:
///
///   * selection is one node per invocation, so a selection of eight is eight
///     runs with a result each. §IV-C of the lab report calls Run "fan-out
///     with a per-node result column", so this is the shape it always wanted.
///   * there is no `snapshot` verb on the console. Stage 11's restore is a
///     verb the node's forced command allows, reached as `run snapshot
///     restore`.
///   * there is no message banner anywhere. `copal fleet notify` means "tell
///     me when all eight are up" and refuses anything else, so Message is not
///     offered rather than offered and broken.
///   * `power on` cannot travel over ssh, because a Pi that is off has no
///     standby rail feeding its NIC. See build_argv.
pub const WRITE_VERBS: &[Verb] = &[
    Verb { name: "scene",    argv: &["scene", "{arg}"],              arg: Arg::Name,  per_node: true },
    Verb { name: "run",      argv: &["run", "{arg}"],                arg: Arg::Name,  per_node: true },
    Verb { name: "power",    argv: &["run", "power", "{arg}"],       arg: Arg::Power, per_node: true },
    Verb { name: "snapshot", argv: &["run", "snapshot", "restore"],  arg: Arg::None,  per_node: true },
    // THE COMMENT ABOVE WAS TRUE UNTIL THE NODE GREW THE VERB. Phase 10 put
    // `message` in copal-fleet-exec and `copal-notify` beside it, so the
    // banner exists and can be offered. It travels as `run message TEXT`,
    // which is how every other forced-command verb travels.
    Verb { name: "message",  argv: &["run", "message", "{arg}"],     arg: Arg::Banner, per_node: true },
];

/// What to ask for, for a verb that needs a word. `None` for a verb that does
/// not -- which is also how `main.rs` decides whether to open the prompt.
pub fn prompt_for(kind: Arg) -> Option<&'static str> {
    match kind {
        Arg::None | Arg::Power => None,
        Arg::Name => Some("name:"),
        Arg::Banner => Some("banner:"),
    }
}

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

/// The banner charset, spelled once here and once in the node's forced
/// command. The test below is what keeps the two spellings the same rule.
fn is_banner(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | ',' | '!' | '?' | ':' | '-'))
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
        // The node's forced command allows `power off` and `power reboot`
        // and nothing else; `on` is handled by build_argv as the supply.
        Arg::Power => match value {
            Some(v @ ("on" | "off" | "reboot")) => Ok(Some(v.to_string())),
            _ => refuse("power takes on, off or reboot"),
        },
        Arg::Name => match value {
            Some(v) if is_name(v) => Ok(Some(v.to_string())),
            other => refuse(format!("not a usable name: {:?}", other.unwrap_or(""))),
        },
        // THE SAME RULE THE NODE ENFORCES, and it has to be: letters, digits,
        // spaces and six marks of punctuation, 200 characters at most.
        // copal-fleet-exec refuses anything else, so a console that allowed
        // more would be a console whose banners sometimes vanish with a
        // refusal nobody could explain. An allow-list rather than a
        // deny-list, so the text cannot carry anything a rendering program
        // might read as markup or a shell as syntax.
        Arg::Banner => match value {
            Some(v) if is_banner(v) => Ok(Some(v.to_string())),
            Some(v) if v.len() > 200 => refuse("a banner is 200 characters or fewer"),
            _ => refuse("a banner is letters, digits and . , ! ? : -"),
        },
    }
}

/// What running a verb actually means: a command, and whether to aim it.
pub struct Plan {
    pub argv: Vec<String>,
    /// When false the command is run once for the whole fleet and `--node` is
    /// never appended.
    pub per_node: bool,
}

/// Build the command for `copal fleet`. No shell, no interpolation, no
/// escaping to get wrong -- each element is pushed whole.
pub fn build_argv(verb: &Verb, arg: Option<&str>) -> Result<Plan, Refused> {
    let arg = validate_arg(verb.arg, arg)?;

    // §9.1, and the one place the table is not enough. A Raspberry Pi that is
    // off has no standby rail feeding its NIC, so nothing on the network can
    // reach it -- `run power on` would be a command sent to a machine that
    // cannot be listening. Switching it on is the supply's job, which is a
    // different verb on the console and is aimed at the fleet file's power
    // configuration rather than at a node.
    if verb.name == "power" && arg.as_deref() == Some("on") {
        return Ok(Plan {
            argv: vec!["power".to_string(), "on".to_string()],
            per_node: false,
        });
    }

    let mut out = Vec::with_capacity(verb.argv.len());
    for tok in verb.argv {
        match *tok {
            "{arg}" => out.push(
                arg.clone()
                    .ok_or_else(|| Refused("this verb needs an argument".into()))?,
            ),
            lit => out.push(lit.to_string()),
        }
    }
    Ok(Plan { argv: out, per_node: verb.per_node })
}

/// The command as it is aimed at one node.
///
/// `--node` goes immediately after the verb word for the same reason `--fleet`
/// does: `cmd_run` swallows everything after the verb's own arguments, so an
/// option placed at the end is eaten rather than read.
pub fn aimed_at(plan: &Plan, node: &str) -> Vec<String> {
    if !plan.per_node || plan.argv.is_empty() {
        return plan.argv.clone();
    }
    let mut argv = vec![plan.argv[0].clone(), "--node".to_string(), node.to_string()];
    argv.extend_from_slice(&plan.argv[1..]);
    argv
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
    }

    #[test]
    fn builds_the_argv_the_real_cli_expects() {
        // `copal fleet scene show --node museum-01`, not `--nodes a,b`.
        let plan = build_argv(lookup("scene").unwrap(), Some("show")).unwrap();
        assert_eq!(plan.argv, ["scene", "show"]);
        assert!(plan.per_node);
        assert_eq!(
            aimed_at(&plan, "museum-01"),
            ["scene", "--node", "museum-01", "show"]
        );
        assert!(!plan.argv.iter().any(|a| a.contains('{')));
    }

    #[test]
    fn a_selection_becomes_one_run_per_node() {
        let plan = build_argv(lookup("snapshot").unwrap(), None).unwrap();
        assert_eq!(plan.argv, ["run", "snapshot", "restore"]);
        for n in ["museum-01", "museum-03"] {
            assert_eq!(
                aimed_at(&plan, n),
                ["run", "--node", n, "snapshot", "restore"],
                "the verb's own arguments must stay together after the options"
            );
        }
    }

    #[test]
    fn power_off_goes_over_ssh_and_power_on_does_not() {
        // §9.1: a Pi that is off has no standby rail feeding its NIC, so `on`
        // is the supply's job and is never aimed at a node.
        let off = build_argv(lookup("power").unwrap(), Some("off")).unwrap();
        assert_eq!(off.argv, ["run", "power", "off"]);
        assert!(off.per_node);

        let on = build_argv(lookup("power").unwrap(), Some("on")).unwrap();
        assert_eq!(on.argv, ["power", "on"]);
        assert!(!on.per_node, "power on was aimed at a node it cannot reach");
        assert_eq!(aimed_at(&on, "museum-07"), ["power", "on"]);
    }

    #[test]
    fn the_banner_verb_exists_now_and_carries_the_node_s_own_rule() {
        // THIS TEST USED TO ASSERT THE OPPOSITE, and the comment explaining
        // why is worth keeping: `copal fleet notify` means "tell me when all
        // eight are up" and refuses anything else, so a banner was not offered
        // rather than offered and broken. Phase 10 put a `message` case in the
        // node's forced command and `copal-notify` beside it, so there is
        // something to call.
        assert!(lookup("message").is_some());
        assert!(lookup("notify").is_none(), "notify is still not a banner");

        // The charset is the node's, spelled again here. Both ends must agree
        // or a banner vanishes with a refusal nobody can explain.
        assert!(validate_arg(Arg::Banner, Some("Please stand back.")).is_ok());
        assert!(validate_arg(Arg::Banner, Some("Closing in 10 minutes - thank you!")).is_ok());
        for bad in [
            "<b>bold</b>",
            "rm -rf /; echo",
            "quote\"inside",
            "back`tick`",
            "$(command)",
            "new\nline",
            "",
        ] {
            assert!(
                validate_arg(Arg::Banner, Some(bad)).is_err(),
                "banner {:?} was allowed and the node would refuse it",
                bad
            );
        }
        let long = "a".repeat(201);
        assert!(validate_arg(Arg::Banner, Some(&long)).is_err());
        assert!(validate_arg(Arg::Banner, Some(&"a".repeat(200))).is_ok());

        // And it travels the way every other forced-command verb travels.
        let plan = build_argv(lookup("message").unwrap(), Some("Please stand back.")).unwrap();
        assert_eq!(plan.argv, vec!["run", "message", "Please stand back."]);
        assert!(plan.per_node, "a banner goes to each node it was aimed at");
    }

    #[test]
    fn control_never_became_a_fanned_out_verb() {
        assert!(lookup("control").is_none());
        assert!(lookup("exchange").is_none());
        assert!(lookup("observe").is_none());
    }
}

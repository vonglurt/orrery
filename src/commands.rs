//! Every command of the window, as a table: what the menu bar shows.
//!
//! Paul, 28 October: "can you make the entire menu of all buttons and goto
//! screen command spawn like a big new from the menus. all options and paths
//! should be in the menu as well." Each row is a menu, a title, a Command key
//! to show, and what it does. A menu item's number is its row; the platform
//! (mac.rs) reports a pick as `Input::Menu(row)`, and main.rs does it.
//!
//! A COMMAND DOES WHAT ITS BUTTON DOES, AND NO MORE. Nothing here goes round
//! a confirmation: a recipe or a join opens the same typed-name screen its
//! button opens; stopping or shutting the fleet down goes to Training with
//! the button armed, waiting for its second press. A command that needs a
//! selection (a Lab verb, a machine) acts on what is selected.

use crate::nav::View;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    /// Go to a view.
    Go(View),
    /// The Training view, at a page (training.rs: 0 Fleet, 1 Runs, 2 Found,
    /// 3 Net, 4 Flags, 5 Candidates).
    Page(usize),
    /// The Sessions grid (no tile enlarged).
    Grid,
    /// A key for the Lab, as if pressed there: its verbs, and Command-A.
    LabKey(char, bool),
    /// A fleet-queue.py command, run as the Training view's buttons run them.
    Queue(&'static str),
    /// The Training button, armed: Collect results and stop now.
    ArmStop,
    /// The Training button, armed: Shut the fleet down.
    ArmShutdown,
    /// A run asked for, as the Runs page's buttons ask.
    Run(&'static str),
    /// The Machines view's, on the selected machine.
    Probe,
    ProbeAll,
    Join,
    Apply(&'static str),
    /// A document in docs/, opened by the system.
    Doc(&'static str),
    /// The platform's own: kept as rows so the table is the whole menu.
    Quit,
    Minimize,
    Close,
    Separator,
}

/// (menu, title, Command key shown, what it does). Order is the menus'.
pub const MENU: &[(&str, &str, &str, Cmd)] = &[
    ("Orrery", "Quit Orrery", "q", Cmd::Quit),
    // ---- the screens
    ("View", "Lab", "1", Cmd::Go(View::Lab)),
    ("View", "Media", "2", Cmd::Go(View::Media)),
    ("View", "Machines", "3", Cmd::Go(View::Machines)),
    ("View", "Training", "4", Cmd::Go(View::Training)),
    ("View", "Sessions", "5", Cmd::Go(View::Sessions)),
    ("View", "-", "", Cmd::Separator),
    ("View", "Training: Fleet", "", Cmd::Page(0)),
    ("View", "Training: Runs", "", Cmd::Page(1)),
    ("View", "Training: Found", "", Cmd::Page(2)),
    ("View", "Training: Candidates", "", Cmd::Page(5)),
    ("View", "Training: Net", "", Cmd::Page(3)),
    ("View", "Training: Flags", "", Cmd::Page(4)),
    ("View", "-", "", Cmd::Separator),
    ("View", "Sessions: Grid", "", Cmd::Grid),
    // ---- the Lab's verbs, on the selected nodes
    ("Lab", "Select All Nodes", "a", Cmd::LabKey('a', true)),
    ("Lab", "-", "", Cmd::Separator),
    ("Lab", "Observe", "", Cmd::LabKey('o', false)),
    ("Lab", "Control", "", Cmd::LabKey('c', false)),
    ("Lab", "Terminal", "", Cmd::LabKey('t', false)),
    ("Lab", "Exchange Files", "", Cmd::LabKey('e', false)),
    ("Lab", "Send", "", Cmd::LabKey('s', false)),
    ("Lab", "Message", "", Cmd::LabKey('m', false)),
    ("Lab", "-", "", Cmd::Separator),
    ("Lab", "Run...", "", Cmd::LabKey('r', false)),
    ("Lab", "Scene...", "", Cmd::LabKey('S', false)),
    ("Lab", "Snapshot", "", Cmd::LabKey('k', false)),
    ("Lab", "Power...", "", Cmd::LabKey('p', false)),
    // ---- the fleet's queue
    ("Fleet", "Sync Now", "", Cmd::Queue("sync")),
    ("Fleet", "Start the Queue", "", Cmd::Queue("ensure")),
    ("Fleet", "Stop Adding", "", Cmd::Queue("stop-adding")),
    ("Fleet", "Resume Adding", "", Cmd::Queue("resume-adding")),
    ("Fleet", "-", "", Cmd::Separator),
    ("Fleet", "Auto Mode On", "", Cmd::Queue("auto on")),
    ("Fleet", "Auto Mode Off", "", Cmd::Queue("auto off")),
    ("Fleet", "Rotation On", "", Cmd::Queue("rotation on")),
    ("Fleet", "Rotation Off", "", Cmd::Queue("rotation off")),
    ("Fleet", "-", "", Cmd::Separator),
    ("Fleet", "Acknowledge All Flags", "", Cmd::Queue("ack all --by operator")),
    ("Fleet", "-", "", Cmd::Separator),
    ("Fleet", "Collect Results and Stop Now...", "", Cmd::ArmStop),
    ("Fleet", "Shut the Fleet Down...", "", Cmd::ArmShutdown),
    // ---- runs, as the Runs page asks them
    ("Runs", "SPSA (search; result adopted)", "", Cmd::Run("new spsa")),
    ("Runs", "Tune (search; result adopted)", "", Cmd::Run("new tune")),
    ("Runs", "Census of @best", "", Cmd::Run("new census trial=@best name=best")),
    ("Runs", "-", "", Cmd::Separator),
    ("Runs", "Parts", "", Cmd::Run("new parts")),
    ("Runs", "Switches", "", Cmd::Run("new switches")),
    ("Runs", "Round Robin", "", Cmd::Run("new roundrobin")),
    ("Runs", "Record", "", Cmd::Run("new record")),
    ("Runs", "-", "", Cmd::Separator),
    ("Runs", "Gauntlet", "", Cmd::Run("new gauntlet")),
    ("Runs", "Train (days)", "", Cmd::Run("new train")),
    ("Runs", "School (days)", "", Cmd::Run("new school")),
    ("Runs", "Clips (this Mac)", "", Cmd::Run("new clips host=mac")),
    ("Runs", "-", "", Cmd::Separator),
    ("Runs", "Families Again", "", Cmd::Run("new families")),
    ("Runs", "Promote the Strongest Now", "", Cmd::Run("new promote candidate=@strongest")),
    // ---- machines, the selected one
    ("Machines", "Probe Selected", "", Cmd::Probe),
    ("Machines", "Probe All", "", Cmd::ProbeAll),
    ("Machines", "Add a Machine...", "", Cmd::Go(View::Machines)),
    ("Machines", "Join Selected...", "", Cmd::Join),
    ("Machines", "-", "", Cmd::Separator),
    ("Machines", "Apply: Make It a Fleet Node...", "", Cmd::Apply("fleet-node")),
    ("Machines", "Apply: CUDA...", "", Cmd::Apply("cuda-node")),
    ("Machines", "Apply: The Login Banner...", "", Cmd::Apply("fleet-login")),
    ("Machines", "Apply: Let the Fleet Power It Off...", "", Cmd::Apply("poweroff")),
    // ---- the window
    ("Window", "Minimize", "m", Cmd::Minimize),
    ("Window", "Close Window", "w", Cmd::Close),
    // ---- the documents
    ("Help", "Orrery User Guide", "", Cmd::Doc("docs/user-guide.html")),
    ("Help", "Fleet Training", "", Cmd::Doc("docs/fleet-train.md")),
    ("Help", "Mac Applications (lab report)", "", Cmd::Doc("docs/mac-app-lab-report.md")),
    ("Help", "From Cargo to AppKit (lab report)", "", Cmd::Doc("docs/appkit-gui-lab-report.md")),
];

/// The menus in order, each once: the table read for the platform.
pub fn menu_titles() -> Vec<&'static str> {
    let mut out: Vec<&str> = Vec::new();
    for (m, ..) in MENU {
        if !out.contains(m) {
            out.push(m);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_and_every_run_kind_is_in_the_menus() {
        for v in View::ALL {
            assert!(MENU.iter().any(|(_, _, _, c)| *c == Cmd::Go(*v)), "{:?} has no menu item", v);
        }
        for kind in ["spsa", "tune", "census", "parts", "switches", "roundrobin", "record", "gauntlet", "train", "school", "clips", "families", "promote"] {
            assert!(
                MENU.iter().any(|(_, _, _, c)| matches!(c, Cmd::Run(v) if v.split_whitespace().nth(1) == Some(kind))),
                "run {} has no menu item",
                kind
            );
        }
    }

    #[test]
    fn a_menu_is_not_split_and_a_key_is_not_given_twice() {
        let titles = menu_titles();
        let mut last = 0;
        for (m, ..) in MENU {
            let i = titles.iter().position(|t| t == m).unwrap();
            assert!(i >= last, "menu {} comes back after another", m);
            last = i;
        }
        let keys: Vec<&str> = MENU.iter().map(|(_, _, k, _)| *k).filter(|k| !k.is_empty()).collect();
        for k in &keys {
            assert_eq!(keys.iter().filter(|x| x == &k).count(), 1, "Command-{} twice", k);
        }
    }
}

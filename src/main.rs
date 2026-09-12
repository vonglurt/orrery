//! orrery -- the Copal fleet's web console.
//!
//! An orrery is a clockwork model that shows many bodies at once and lets you
//! turn a handle to see where they will be. That is the wall, and the name is
//! the theme's own vocabulary rather than a description of a web server.
//!
//! §12 of `docs/fleet-plan.md` names three faces on one read model and gives
//! this one its job: "the gallery screen, and the phone in the operator's
//! pocket -- third, served by the warden, read-mostly." This is that face, plus
//! the half of `docs/fleet-lab-report.md` that was never built: Control,
//! Observe, Exchange, Send and Message are dimmed in the TUI's footer with the
//! words "L6, not built", and they are the Timbuktu lineage the whole survey
//! was written around.
//!
//! THE ARCHITECTURAL RULE IS THE TUI'S RULE, VERBATIM. `copal-fleet-console.py`
//! opens no socket, holds no credential and knows no subject names; it shells
//! out to `copal fleet` for its picture and its verbs. This does the same and
//! for the same reason -- a console that learns to talk to nodes directly has
//! become a second way of knowing things, and §12's ordering exists to stop
//! that. Every screen here is a rendering of a command a person could type.
//!
//! READ-MOSTLY IS A DEFAULT, NOT A CEILING. A gallery screen and a phone on a
//! lanyard are two audiences with different rights, so there are two postures:
//!
//!     gallery   (default)  state, thumbnails, Observe. No verb that changes
//!                          anything. This is what §12 meant.
//!     operator  (--operator TOKEN)  the write verbs as well. Without a token
//!                          the write route does not exist rather than
//!                          answering 403 -- a route that says "forbidden"
//!                          tells a scanner it is there.

// The primitives, and the transport built on them. Parts of both are written
// for phases that have not arrived -- AES-GCM and DER are TLS's and the
// certificate writer's -- so they are dead code today and deliberately so.
// `nothing_here_is_off_the_profile` is what keeps the set honest in the
// meantime: nothing is in there that the whitelist does not name.
#[allow(dead_code)]
mod crypto;
mod draw;
#[allow(dead_code)]
mod files;
mod fleet;
mod font;
mod http;
mod json;
mod keymap;
mod lab;
mod media;
mod nav;
#[cfg(target_os = "macos")]
mod mac;
mod paint;
mod profile;
mod pty;
#[allow(dead_code)]
mod rdp;
mod rfb;
#[allow(dead_code)]
mod screen;
mod seat;
#[allow(dead_code)]
mod sftp;
#[allow(dead_code)]
mod ssh;
mod surface;
#[allow(dead_code)]
mod tls;
mod ui;
#[allow(dead_code)]
mod x509;
mod verbs;
// The native GUI. Wayland is a Linux protocol and `sys.rs` declares Linux
// system calls, so both are absent elsewhere -- the seat and the wall still
// build on a developer's Mac, which is where most of this is written.
#[cfg(target_os = "linux")]
mod sys;
#[cfg(target_os = "linux")]
mod wl;

use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use fleet::Fleet;
use http::{Request, Response};

/// The page. One file, no CDN, no build step, no toolchain to change it with.
const PAGE: &str = include_str!("../assets/wall.html");

struct Console {
    fleet: Fleet,
    /// Empty means the gallery posture and no write route at all.
    operator: String,
    quiet: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        usage();
        return;
    }

    let mut listen = "127.0.0.1:8080".to_string();
    let mut fleet_name = String::new();
    let mut fleet_cmd = "copal fleet".to_string();
    let mut operator = String::new();
    let mut demo = false;
    let mut quiet = false;
    let mut seat = String::new();
    let mut seat_opts = seat::Opts::default();
    let mut gui = false;
    let mut frame_to = String::new();
    let mut dark = false;
    let mut copal = String::new();
    let mut ssh_user = String::from("copal");
    let mut zoom = 0usize;
    let mut specimen_frame = false;
    let mut frame_view = String::new();
    let mut frame_select: Vec<String> = Vec::new();
    let mut frame_state = String::new();
    let mut frame_size = (960usize, 600usize);

    let mut i = 0;
    while i < args.len() {
        let need = |i: usize, what: &str| -> String {
            args.get(i + 1).cloned().unwrap_or_else(|| {
                eprintln!("orrery: {} needs a value", what);
                std::process::exit(2);
            })
        };
        match args[i].as_str() {
            "--listen" => { listen = need(i, "--listen"); i += 2 }
            "--fleet" => { fleet_name = need(i, "--fleet"); i += 2 }
            "--fleet-cmd" => { fleet_cmd = need(i, "--fleet-cmd"); i += 2 }
            "--operator" => { operator = need(i, "--operator"); i += 2 }
            "--seat" => { seat = need(i, "--seat"); i += 2 }
            "--vnc-port" => {
                let v = need(i, "--vnc-port");
                seat_opts.port = match v.parse() {
                    Ok(p) => p,
                    Err(_) => { eprintln!("orrery: --vnc-port takes a number"); std::process::exit(2) }
                };
                i += 2
            }
            "--fps" => {
                let v = need(i, "--fps");
                seat_opts.fps = match v.parse::<u32>() {
                    Ok(f) if f >= 1 && f <= 60 => f,
                    _ => { eprintln!("orrery: --fps takes 1 to 60"); std::process::exit(2) }
                };
                i += 2
            }
            "--gui" => { gui = true; i += 1 }
            "--frame" => { frame_to = need(i, "--frame"); i += 2 }
            // The review flags. A console whose screens can only be seen by
            // standing in front of one is a console nobody reviews.
            "--specimen" => { specimen_frame = true; i += 1 }
            "--frame-view" => { frame_view = need(i, "--frame-view"); i += 2 }
            "--frame-state" => { frame_state = need(i, "--frame-state"); i += 2 }
            "--frame-select" => {
                frame_select = need(i, "--frame-select")
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                i += 2
            }
            "--frame-size" => {
                let v = need(i, "--frame-size");
                let (a, b) = v.split_once('x').unwrap_or(("", ""));
                match (a.parse::<usize>(), b.parse::<usize>()) {
                    (Ok(w), Ok(h)) if w >= 120 && h >= 120 && w <= 8192 && h <= 8192 => {
                        frame_size = (w, h)
                    }
                    _ => {
                        eprintln!("orrery: --frame-size takes WIDTHxHEIGHT, e.g. 960x600");
                        std::process::exit(2)
                    }
                }
                i += 2
            }
            "--dark" => { dark = true; i += 1 }
            "--copal" => { copal = need(i, "--copal"); i += 2 }
            // The account on the node -- `copal-prep.sh`'s PI_USER. It is a
            // flag rather than a guess because the certificate names
            // principals, not usernames, and which account those principals
            // are listed under is the node's decision.
            "--user" => { ssh_user = need(i, "--user"); i += 2 }
            "--sixel" => { seat_opts.paint = Some(paint::Mode::Sixel); i += 1 }
            "--half-block" => { seat_opts.paint = Some(paint::Mode::HalfBlock); i += 1 }
            "--demo" => { demo = true; i += 1 }
            "--quiet" => { quiet = true; i += 1 }
            // HOW BIG A DRAWN PIXEL IS, in screen pixels, per side.
            //
            // The interface is drawn in 8x13 and 10x20 bitmap glyphs, and the
            // canvas is allocated in DEVICE pixels -- so on a Retina panel or
            // a 4K screen every letter comes out half the size it was designed
            // at. Scaling the glyphs is not an option: they are bitmaps, and a
            // bitmap font at 1.5x is a smear. So the whole interface is drawn
            // smaller and each pixel is repeated, at an integer factor, nearest
            // neighbour. `--scale 0` (the default) means whatever the panel
            // needs: the Mac's backing scale factor, and 1 on Wayland.
            "--scale" => {
                let v = need(i, "--scale");
                match v.parse::<usize>() {
                    Ok(n) if n <= 6 => zoom = n,
                    _ => {
                        eprintln!("orrery: --scale takes 0 to 6 (0 means whatever the panel needs)");
                        std::process::exit(2)
                    }
                }
                i += 2
            }
            // The node's sshd lines, rendered from the same whitelist the
            // client builds its offer from. This is how the two ends stay in
            // step: `copal-prep.sh` writes what this prints, so a profile that
            // narrows here narrows there in the same commit rather than in a
            // later one somebody has to remember.
            "--profile-sshd" => {
                print!("{}", profile::sshd_config(&profile::P1));
                std::process::exit(0);
            }
            other => {
                eprintln!("orrery: unknown option {:?}", other);
                usage();
                std::process::exit(2);
            }
        }
    }

    let cmd: Vec<String> = fleet_cmd.split_whitespace().map(str::to_string).collect();
    if cmd.is_empty() {
        eprintln!("orrery: --fleet-cmd is empty");
        std::process::exit(2);
    }

    // Without --demo the CLI is the whole read model, so its absence is worth
    // one clear sentence at startup rather than a wall of identical errors
    // once the page is open.
    // THE EXEMPTION IS OFF AGAIN, AS IT SAID IT WOULD BE. --gui was exempt
    // while it painted a specimen and had no read model to be missing. Phase 3
    // moved the wall into it, so it has one, so its absence is worth the same
    // clear sentence at startup as every other face gets.
    if !demo && frame_to.is_empty() && !on_path(&cmd[0]) {
        eprintln!("orrery: {} is not on PATH -- try --demo", cmd[0]);
        std::process::exit(2);
    }

    // --frame renders one frame to a file and exits. It needs no compositor,
    // which makes it the way to look at the drawing on a machine that has
    // none -- a headless node, a container, a developer's Mac.
    if !frame_to.is_empty() {
        let theme = if dark { draw::DARK } else { draw::LIGHT };
        let what = if specimen_frame {
            Frame::Specimen
        } else if frame_view == "files" {
            Frame::Files {
                nodes: if frame_select.is_empty() {
                    vec!["museum-01".to_string()]
                } else {
                    frame_select.clone()
                },
            }
        } else if frame_view == "media" {
            Frame::Media { copal: copal.clone(), demo }
        } else {
            // The document comes from a file, from the fixture, or from the
            // CLI -- in that order, so that a review frame can be pinned to a
            // saved document and stay the same picture next week.
            let doc = if !frame_state.is_empty() {
                match std::fs::read_to_string(&frame_state) {
                    Ok(d) => fleet::annotate(&d),
                    Err(e) => {
                        eprintln!("orrery: cannot read {}: {}", frame_state, e);
                        std::process::exit(1);
                    }
                }
            } else if demo || !on_path(&cmd[0]) {
                fleet::annotate(&fleet::demo_doc())
            } else {
                let f = Fleet::new(cmd.clone(), fleet_name.clone(), demo);
                match f.state() {
                    Ok(d) => fleet::annotate(&d),
                    Err(e) => {
                        eprintln!("orrery: {}", e);
                        std::process::exit(1);
                    }
                }
            };
            Frame::Lab {
                doc,
                select: frame_select.clone(),
                posture: if operator.is_empty() {
                    lab::Posture::Gallery
                } else {
                    lab::Posture::Operator
                },
            }
        };
        if let Err(e) = write_frame(&frame_to, frame_size.0, frame_size.1, &theme, what) {
            eprintln!("orrery: {}", e);
            std::process::exit(1);
        }
        eprintln!("orrery: wrote {}", frame_to);
        return;
    }

    // --gui is phase 1 of the native console: a window, painted, resizable,
    // closable. The wall and the seat move into it in later phases; what this
    // proves is that the Wayland client underneath works at all.
    if gui {
        let theme = if dark { draw::DARK } else { draw::LIGHT };
        let posture = if operator.is_empty() {
            lab::Posture::Gallery
        } else {
            lab::Posture::Operator
        };
        let fleet = Arc::new(Fleet::new(cmd.clone(), fleet_name.clone(), demo));

        #[cfg(target_os = "macos")]
        {
            let win = match mac::Window::open("orrery", 960, 600, zoom) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("orrery: {}", e);
                    std::process::exit(1);
                }
            };
            eprintln!(
                "orrery: window open, backing scale {}  posture: {}{}",
                win.scale(),
                if operator.is_empty() { "gallery (read-only)" } else { "operator" },
                if demo { "  [demo fixture]" } else { "" }
            );
            if let Err(e) = gui_loop(win, theme, fleet, posture, &copal, &ssh_user, demo) {
                eprintln!("orrery: {}", e);
                std::process::exit(1);
            }
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let win = match wl::Window::open("orrery", 960, 600, zoom) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("orrery: {}", e);
                    std::process::exit(1);
                }
            };
            eprintln!(
                "orrery: window open at {}x{}  posture: {}{}",
                win.width,
                win.height,
                if operator.is_empty() { "gallery (read-only)" } else { "operator" },
                if demo { "  [demo fixture]" } else { "" }
            );
            if let Err(e) = gui_loop(win, theme, fleet, posture, &copal, &ssh_user, demo) {
                eprintln!("orrery: {}", e);
                std::process::exit(1);
            }
            return;
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            eprintln!(
                "orrery: --gui needs Wayland or AppKit. This binary was built for {}.",
                std::env::consts::OS
            );
            std::process::exit(2);
        }
    }

    // --seat is a different program wearing the same binary: no listener, no
    // routes, one node. It shares the read model and nothing else, which is
    // the whole reason it lives here rather than in a second crate.
    if !seat.is_empty() {
        let fleet = Fleet::new(cmd, fleet_name, demo);
        if let Err(e) = seat::run(&fleet, &seat, seat_opts) {
            eprintln!("orrery: {}", e);
            std::process::exit(1);
        }
        return;
    }

    let console = Arc::new(Console {
        fleet: Fleet::new(cmd, fleet_name, demo),
        operator,
        quiet,
    });

    let listener = match TcpListener::bind(&listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("orrery: cannot listen on {}: {}", listen, e);
            std::process::exit(1);
        }
    };

    eprintln!(
        "orrery: http://{}/  posture: {}{}",
        listen,
        if console.operator.is_empty() { "gallery (read-only)" } else { "operator" },
        if demo { "  [demo fixture]" } else { "" }
    );

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let console = Arc::clone(&console);
                // A thread per connection, and every response closes it. Eight
                // tiles polling every five seconds is not a workload that
                // needs an executor, and an executor is a dependency.
                thread::spawn(move || serve(&console, stream));
            }
            Err(e) if !console.quiet => eprintln!("orrery: accept failed: {}", e),
            Err(_) => {}
        }
    }
}

/// What `--frame` should render.
enum Frame {
    /// Phase 2's palette sheet: every primitive in `draw.rs`, once.
    Specimen,
    /// The museum interface, against a document.
    Lab {
        doc: String,
        select: Vec<String>,
        posture: lab::Posture,
    },
    /// The card ledger, against a checkout.
    Media { copal: String, demo: bool },
    /// The file browser, against this machine and a made-up node.
    ///
    /// The remote side is a fixture rather than a connection, for the same
    /// reason `--demo` exists at all: the drawing has to be reviewable without
    /// eight Raspberry Pis on the bench.
    Files { nodes: Vec<String> },
}

/// THE SPECIMEN, which is phase 2's deliverable and its own regression test.
///
/// Every primitive in `draw.rs` appears here once: both faces, the palette,
/// filled and hollow discs, a frame, a mixed gradient, right-aligned numbers
/// and a string deliberately too long for its column. When the wall arrives in
/// phase 3 it is built out of these, and until then this is how a change to
/// the canvas gets looked at rather than merely compiled.
fn specimen(c: &mut draw::Canvas, t: &draw::Theme) {
    use draw::mix;
    use font::{F10X20, F8X13};

    let w = c.w as i32;
    c.clear(t.base);

    // The header, in the large face.
    c.rect(0, 0, w, 34, t.panel);
    c.hline(0, 33, w, t.line);
    let at = c.text(12, 7, "COPAL FLEET ", &F10X20, t.ink);
    let at = c.text(at, 7, "\u{b7} ", &F10X20, t.dim);
    c.text(at, 7, "MUSEUM", &F10X20, t.accent);
    c.text_right(w - 12, 11, "6 of 8 up", &F8X13, t.dim);

    // A row of tiles, which is the wall's actual unit.
    let nodes = [
        ("museum-01", "show", 45u32, 0u8),
        ("museum-02", "show", 51, 0),
        ("museum-06", "wake", 39, 1),
        ("museum-07", "not announced", 0, 2),
    ];
    let tw = 216;
    let th = 96;
    for (i, (id, scene, temp, state)) in nodes.iter().enumerate() {
        let x = 12 + i as i32 * (tw + 10);
        let y = 48;
        c.rect(x, y, tw, th, t.tile);
        c.frame(x, y, tw, th, t.line);

        // The status light: filled when the node is there, hollow when it is
        // only declared. The SHAPE carries the state as well as the colour,
        // so it survives being looked at from across a gallery.
        let colour = match state {
            0 => t.up,
            1 => t.warn,
            _ => t.dim,
        };
        if *state == 2 {
            c.ring(x + 14, y + 15, 4, colour);
        } else {
            c.disc(x + 14, y + 15, 4, colour);
        }
        c.text(x + 26, y + 9, id, &F8X13, t.ink);

        // The scene, clipped rather than allowed to run into the next tile.
        c.rect(x + 10, y + 30, tw - 20, 26, t.panel);
        c.text_in(x + 16, y + 36, tw - 32, scene, &F8X13, t.dim);

        if *temp > 0 {
            // A quantity, so it gets a bar as well as a number: 30 degrees is
            // cool, 70 is hot, and the colour says which end it is nearer.
            let frac = (((*temp as i32 - 30).clamp(0, 40)) * 255 / 40) as u8;
            let bar = mix(t.up, t.alarm, frac);
            let full = tw - 32;
            let filled = full * frac as i32 / 255;
            c.rect(x + 16, y + 66, full, 6, t.panel);
            c.rect(x + 16, y + 66, filled, 6, bar);
            c.text(x + 16, y + 78, "agent 1s", &F8X13, t.dim);
            c.text_right(x + tw - 16, y + 78, &format!("{}\u{b0}C", temp), &F8X13, t.dim);
        } else {
            c.text_right(x + tw - 16, y + 78, "last seen 08:12", &F8X13, t.dim);
        }
    }

    // The type specimen proper, so a font change is visible.
    let mut y = 164;
    c.text(12, y, "10x20", &F8X13, t.accent);
    y += 17;
    c.text(12, y, "ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789", &F10X20, t.ink);
    y += 22;
    c.text(12, y, "abcdefghijklmnopqrstuvwxyz .,:;!?-+/", &F10X20, t.ink);
    y += 30;
    c.text(12, y, "8x13", &F8X13, t.accent);
    y += 16;
    c.text(12, y, "ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789", &F8X13, t.ink);
    y += 15;
    c.text(12, y, "abcdefghijklmnopqrstuvwxyz .,:;!?-+/", &F8X13, t.ink);
    y += 15;
    c.text(12, y, "45\u{b0}C \u{b7} scene show \u{b7} agent 1s \u{b7} warden", &F8X13, t.dim);
    y += 15;
    // EVERY CHARACTER BAKED PAST LATIN-1, at both sizes. `font.rs` returns a
    // blank cell for anything it does not carry, which is right and which
    // means a missing glyph is invisible until somebody looks -- and this is
    // the sheet somebody looks at. A button once shipped reading "copy" where
    // it should have read "← copy" for exactly this reason.
    c.text(12, y, "extras  \u{2192} \u{2190} \u{2014} \u{2026} \u{25cf} \u{25cb} \u{2580}", &F8X13, t.ink);
    y += 22;
    c.text(12, y, "\u{2192} \u{2190} \u{2014} \u{2026} \u{25cf} \u{25cb} \u{2580}", &F10X20, t.ink);

    // The palette, named -- a swatch nobody can name is decoration.
    y += 26;
    c.text(12, y, "palette", &F8X13, t.accent);
    y += 15;
    let swatches: [(&str, draw::Rgb); 8] = [
        ("base", t.base),
        ("panel", t.panel),
        ("tile", t.tile),
        ("line", t.line),
        ("ink", t.ink),
        ("dim", t.dim),
        ("up", t.up),
        ("alarm", t.alarm),
    ];
    for (i, (name, colour)) in swatches.iter().enumerate() {
        let x = 12 + i as i32 * 104;
        c.rect(x, y, 92, 26, *colour);
        c.frame(x, y, 92, 26, t.line);
        c.text(x, y + 31, name, &F8X13, t.dim);
    }

    // Clipping, shown rather than only asserted: this string does not fit its
    // column, and the marker is what says so.
    y += 62;
    c.text(12, y, "clipped to its column:", &F8X13, t.dim);
    c.rect(190, y - 4, 124, 19, t.panel);
    c.frame(190, y - 4, 124, 19, t.line);
    c.text_in(194, y, 116, "museum-01.gallery.local", &F8X13, t.ink);

    // And the same string with room, so the two read against each other.
    c.text(330, y, "museum-01.gallery.local", &F8X13, t.ink);
}

/// One frame to a binary PPM, which every viewer reads and which needs no
/// encoder, no library and no compositor.
///
/// THIS IS HOW THE INTERFACE IS REVIEWED. Every screen in `docs/console.md` is
/// a file this produces, so a change to the drawing is a diff rather than a
/// thing somebody has to stand in front of a node to notice. `--specimen`
/// still renders phase 2's palette sheet, which is the regression test for
/// `draw.rs` itself.
fn write_frame(
    path: &str,
    w: usize,
    h: usize,
    theme: &draw::Theme,
    what: Frame,
) -> Result<(), String> {
    use std::io::Write;

    let mut px = vec![0u8; w * h * 4];
    {
        let mut c = draw::Canvas::new(&mut px, w, h);
        match what {
            Frame::Specimen => specimen(&mut c, theme),
            Frame::Media { ref copal, demo } => {
                let mut m = if demo {
                    media::Media::demo()
                } else {
                    media::Media::open(copal)
                };
                let mut ui_state = ui::UiState::default();
                ui_state.pointer = (-1, -1);
                let mut u = ui::Ui::begin(&mut c, *theme, &[], &mut ui_state);
                media::draw(&mut u, &mut m);
            }
            Frame::Files { ref nodes } => {
                let here = std::env::current_dir().unwrap_or_else(|_| ".".into());
                let mut f = files::Files::specimen(nodes.clone(), here);
                let mut ui_state = ui::UiState::default();
                ui_state.pointer = (-1, -1);
                let mut u = ui::Ui::begin(&mut c, *theme, &[], &mut ui_state);
                files::draw(&mut u, ui::rect(0, 0, w as i32, h as i32), &mut f);
            }
            Frame::Lab { ref doc, ref select, posture } => {
                let model = lab::Model::parse(doc);
                let mut state = lab::Lab::new(posture);
                // `--frame-verb NAME` draws the bar as the prompt that verb
                // opens, which is the only way to review a question that only
                // exists between two clicks.
                if let Some(verb) = std::env::var("ORRERY_FRAME_VERB").ok().filter(|v| !v.is_empty())
                {
                    if let Some(face) = lab::FACES.iter().find(|f| f.name == verb) {
                        state.ask = Some(lab::Ask {
                            verb: face.name,
                            nodes: select.clone(),
                            field: ui::Field::new(if face.name == "Message" {
                                "please stand back"
                            } else {
                                "show"
                            }),
                            prompt: verbs::lookup(&face.name.to_ascii_lowercase())
                                .and_then(|v| verbs::prompt_for(v.arg))
                                .unwrap_or("word:"),
                        });
                    }
                }
                for id in select {
                    if model.index_of(id).is_some() {
                        state.selection.push(id.clone());
                    }
                }
                let mut ui_state = ui::UiState::default();
                // Off the canvas, so nothing is drawn hot. A review frame
                // should show the interface at rest, not mid-hover.
                ui_state.pointer = (-1, -1);
                let mut u = ui::Ui::begin(&mut c, *theme, &[], &mut ui_state);
                lab::draw(&mut u, &model, &mut state);
            }
        }
    }

    let mut out = Vec::with_capacity(w * h * 3 + 32);
    out.extend_from_slice(format!("P6\n{} {}\n255\n", w, h).as_bytes());
    for chunk in px.chunks_exact(4) {
        let v = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        out.push((v >> 16) as u8);
        out.push((v >> 8) as u8);
        out.push(v as u8);
    }
    std::fs::File::create(path)
        .and_then(|mut f| f.write_all(&out))
        .map_err(|e| format!("cannot write {}: {}", path, e))
}

/// The console, on whatever surface the platform gave us.
///
/// ONE LOOP FOR BOTH PLATFORMS, which is the whole return on `surface.rs`
/// existing. `mac.rs` and `wl.rs` differ in how a rectangle of memory is got
/// and how an event is heard; from here down neither is visible.
fn gui_loop<S: surface::Surface>(
    mut s: S,
    theme: draw::Theme,
    fleet: Arc<Fleet>,
    posture: lab::Posture,
    copal: &str,
    // The account on the node. See `--user`.
    ssh_user: &str,
    demo_fixture: bool,
) -> Result<(), String> {
    use std::time::Duration;

    // THE READ MODEL IS REFRESHED ON A SECOND THREAD, and it has to be.
    // `copal fleet state` may take forty-five seconds, and a console that
    // stops drawing while it asks is worse than one showing a five-second-old
    // picture. The thread posts a document; the loop takes whatever is there.
    let doc = Arc::new(Mutex::new(String::new()));
    {
        let doc = Arc::clone(&doc);
        let fleet = Arc::clone(&fleet);
        thread::spawn(move || loop {
            let text = match fleet.state() {
                Ok(d) => fleet::annotate(&d),
                Err(e) => format!("{{\"error\":{}}}", json::Value::quote(&e)),
            };
            *doc.lock().unwrap_or_else(|e| e.into_inner()) = text;
            thread::sleep(Duration::from_secs(5));
        });
    }

    // A verb may take three minutes, so it runs on a thread too and the pane
    // says "running" until it lands.
    let results: Arc<Mutex<Option<lab::Results>>> = Arc::new(Mutex::new(None));

    // A shell on a node. It lives here rather than in `lab.rs` for the same
    // reason the verbs do: lab.rs draws, and opening a socket is not drawing.
    // Connecting takes a second or two -- a key exchange, a certificate check
    // and an authentication -- so it happens on a thread and the pane appears
    // when it is ready.
    let mut shell: Option<media::Job> = None;
    // The file browser. Exchange and Send are the same pane aimed at one node
    // or at several, so there is one of these rather than two.
    let mut browser: Option<files::Files> = None;
    // A node's desktop. One at a time -- console.md §III-B priced eight live
    // sessions on 512 MB boards and refused them, and the verb table says
    // Control is Arity::One for the same reason.
    let mut desktop: Option<screen::Screen> = None;
    let opening_desktop: Arc<Mutex<Option<Result<screen::Screen, String>>>> =
        Arc::new(Mutex::new(None));
    let opening: Arc<Mutex<Option<Result<media::Job, String>>>> = Arc::new(Mutex::new(None));
    let mut connecting: Option<String> = None;
    let mut shell_size = (0u32, 0u32);

    let mut ui_state = ui::UiState::default();
    let mut lab_state = lab::Lab::new(posture);
    let mut view = nav::View::default();
    // The card pane reads files rather than the fleet, so it is opened once
    // and reloaded after anything that writes one.
    let mut media = if demo_fixture {
        media::Media::demo()
    } else {
        media::Media::open(copal)
    };
    let frame = Duration::from_millis(1000 / 12);

    loop {
        let inputs = s.poll()?;
        if s.closed() || inputs.iter().any(|i| *i == surface::Input::Closed) {
            return Ok(());
        }
        if inputs.iter().any(|i| matches!(i, surface::Input::Resized { .. })) {
            s.apply_resize()?;
        }

        let text = doc.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let model = lab::Model::parse(if text.is_empty() { "{}" } else { &text });
        lab_state.reconcile(&model);
        lab_state.results = results.lock().unwrap_or_else(|e| e.into_inner()).clone();

        // A running card write is pumped every frame whichever view is in
        // front, so switching to the Lab while a card is being written does
        // not stop reading from the terminal.
        if let Some(j) = media.job.as_mut() {
            j.pump();
        }
        if let Some(j) = shell.as_mut() {
            j.pump();
        }
        if let Some(b) = browser.as_mut() {
            b.pump();
        }
        if let Some(d) = desktop.as_mut() {
            d.pump();
        }
        if let Some(done) = opening_desktop
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            connecting = None;
            match done {
                Ok(scr) => {
                    desktop = Some(scr);
                    *results.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    lab_state.results = None;
                }
                Err(e) => {
                    let node = lab_state.selection.first().cloned().unwrap_or_default();
                    *results.lock().unwrap_or_else(|err| err.into_inner()) = Some(lab::Results {
                        verb: "Control".to_string(),
                        running: false,
                        items: vec![lab::Outcome { node, code: 1, output: e }],
                    });
                }
            }
        }
        if let Some(done) = opening.lock().unwrap_or_else(|e| e.into_inner()).take() {
            connecting = None;
            match done {
                Ok(j) => {
                    shell = Some(j);
                    // The "connecting" pane has served its purpose; leaving it
                    // set would put it behind the shell, waiting to reappear.
                    *results.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    lab_state.results = None;
                }
                // A refusal is the interesting case and it is shown in the
                // results pane, which is where every other verb's failure
                // already goes -- a certificate that did not check out is a
                // sentence, not a silence.
                Err(e) => {
                    let node = lab_state.selection.first().cloned().unwrap_or_default();
                    *results.lock().unwrap_or_else(|err| err.into_inner()) = Some(lab::Results {
                        verb: "Terminal".to_string(),
                        running: false,
                        items: vec![lab::Outcome { node, code: 1, output: e }],
                    });
                }
            }
        }
        media.note_fleet(&model.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>());

        let (w, h) = s.size();
        let mut action = None;
        let mut close_shell = false;
        let mut close_browser = false;
        let mut close_desktop = false;
        {
            let mut c = draw::Canvas::new(s.pixels(), w, h);
            let mut u = ui::Ui::begin(&mut c, theme, &inputs, &mut ui_state);
            match (shell.as_mut(), view) {
                // A SHELL IS IN FRONT OF EVERYTHING. It is the only face that
                // takes every keystroke, so leaving the lab's own keys live
                // underneath it would mean typing `p` at a prompt powered a
                // node off.
                (Some(j), _) => {
                    let at = ui::rect(0, 0, w as i32, h as i32);
                    let want = media::pane_size(at);
                    if want != shell_size {
                        j.resize(want.0, want.1);
                        shell_size = want;
                    }
                    match media::draw_pane(&mut u, at, j) {
                        media::Pane::Stop => j.stop(),
                        media::Pane::Close => close_shell = true,
                        media::Pane::Stay => {}
                    }
                }
                (None, _) if desktop.is_some() => {
                    let d = desktop.as_mut().expect("checked");
                    let at = ui::rect(0, 0, w as i32, h as i32);
                    if screen::draw(&mut u, at, d) == screen::Pane::Close {
                        close_desktop = true;
                    }
                }
                (None, _) if browser.is_some() => {
                    let b = browser.as_mut().expect("checked");
                    let at = ui::rect(0, 0, w as i32, h as i32);
                    files::keys(&mut u, b);
                    if files::draw(&mut u, at, b) {
                        close_browser = true;
                    }
                }
                (None, nav::View::Lab) => action = lab::draw(&mut u, &model, &mut lab_state),
                (None, nav::View::Media) => {
                    if let Some(v) = media::draw(&mut u, &mut media) {
                        action = Some(lab::Action::Go(v));
                    }
                }
            }
        }
        if let Some(lab::Action::Go(v)) = action {
            view = v;
            if v == nav::View::Media {
                media.reload();
            }
            action = None;
        }
        // A dismissed pane has to be cleared where it is OWNED, or the next
        // frame copies it straight back out of the mutex.
        if lab_state.results.is_none() {
            *results.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        s.present()?;

        if close_shell {
            shell = None;
            shell_size = (0, 0);
        }
        if close_browser {
            browser = None;
        }
        if close_desktop {
            if let Some(d) = desktop.as_mut() {
                d.session.close();
            }
            desktop = None;
        }
        if let Some(lab::Action::Typed { name, nodes, arg }) = action {
            run_verb(name, nodes, Some(arg), Arc::clone(&fleet), Arc::clone(&results));
        } else if let Some(lab::Action::Verb { name, nodes }) = action {
            if name == "Terminal" {
                // One node, because a shell is a conversation with one
                // machine -- `Arity::One` in the verb table already says so,
                // and this is the second place that has to agree.
                if let Some(node) = nodes.first().cloned() {
                    if shell.is_none() && connecting.is_none() {
                        let address = model
                            .nodes
                            .iter()
                            .find(|n| n.id == node)
                            .map(|n| n.address.clone())
                            .unwrap_or_default();
                        connecting = Some(node.clone());
                        // The same "running" pane every slow verb shows. A
                        // handshake takes a second or two and a console that
                        // looked frozen for it would be a console people click
                        // twice.
                        *results.lock().unwrap_or_else(|e| e.into_inner()) = Some(lab::Results {
                            verb: "Terminal".to_string(),
                            running: true,
                            items: Vec::new(),
                        });
                        open_shell(
                            model.fleet.clone(),
                            node,
                            address,
                            ssh_user.to_string(),
                            Arc::clone(&opening),
                        );
                    }
                }
            } else if name == "Control" {
                if let Some(node) = nodes.first().cloned() {
                    // WHAT THE NODE SAYS ABOUT ITS OWN SCREEN, rather than a
                    // port scan. `remote` is one of off, vnc:PORT, rdp:PORT or
                    // "not installed", and each of those is a different
                    // sentence -- the useless one being the connection attempt
                    // that times out because nothing was ever listening.
                    let said = model
                        .nodes
                        .iter()
                        .find(|n| n.id == node)
                        .and_then(|n| n.remote.clone())
                        .unwrap_or_default();
                    let refusal = match said.as_str() {
                        s if s.starts_with("rdp") => None,
                        "" => None, // has not said; try, and let the socket answer
                        "off" => Some(
                            "that node's screen is not on the network.                              Run `copal fleet run remote start` on it first."
                                .to_string(),
                        ),
                        "not installed" => Some(
                            "that node has no remote-desktop server installed -- see                              fleet-control.md H1."
                                .to_string(),
                        ),
                        s if s.starts_with("vnc") => Some(format!(
                            "that node serves RFB ({}), and this window's Control speaks RDP.                              The seat speaks RFB today: orrery --seat {}",
                            s, node
                        )),
                        s => Some(format!("that node says its screen is {:?}", s)),
                    };
                    if let Some(why) = refusal {
                        *results.lock().unwrap_or_else(|e| e.into_inner()) = Some(lab::Results {
                            verb: "Control".to_string(),
                            running: false,
                            items: vec![lab::Outcome { node: node.clone(), code: 1, output: why }],
                        });
                    } else if desktop.is_none() && shell.is_none() && connecting.is_none() {
                        let address = model
                            .nodes
                            .iter()
                            .find(|n| n.id == node)
                            .map(|n| n.address.clone())
                            .unwrap_or_default();
                        connecting = Some(node.clone());
                        *results.lock().unwrap_or_else(|e| e.into_inner()) = Some(lab::Results {
                            verb: "Control".to_string(),
                            running: true,
                            items: Vec::new(),
                        });
                        open_desktop(
                            model.fleet.clone(),
                            node,
                            address,
                            ssh_user.to_string(),
                            Arc::clone(&opening_desktop),
                        );
                    }
                }
            } else if name == "Exchange" || name == "Send" {
                if browser.is_none() && shell.is_none() {
                    match dials_for(&model, &nodes, ssh_user) {
                        Ok(dials) => {
                            let start = std::env::current_dir().unwrap_or_else(|_| {
                                std::path::PathBuf::from(
                                    std::env::var("HOME").unwrap_or_else(|_| "/".into()),
                                )
                            });
                            browser = Some(files::Files::open(dials, start));
                        }
                        // A missing operator key or certificate is the usual
                        // reason, and it is the same sentence for every verb
                        // that needs one.
                        Err(e) => {
                            *results.lock().unwrap_or_else(|err| err.into_inner()) =
                                Some(lab::Results {
                                    verb: name.to_string(),
                                    running: false,
                                    items: vec![lab::Outcome {
                                        node: nodes.first().cloned().unwrap_or_default(),
                                        code: 1,
                                        output: e,
                                    }],
                                });
                        }
                    }
                }
            } else if let Some(prompt) = verbs::lookup(&name.to_ascii_lowercase())
                .and_then(|v| verbs::prompt_for(v.arg))
            {
                // It needs a word. The bar becomes the question rather than
                // the verb running with a guess.
                lab_state.ask = Some(lab::Ask {
                    verb: name,
                    nodes,
                    field: ui::Field::default(),
                    prompt,
                });
            } else {
                run_verb(name, nodes, None, Arc::clone(&fleet), Arc::clone(&results));
            }
        }

        thread::sleep(frame);
    }
}

/// The key material for each selected node, or the first sentence that
/// explains why there is none.
///
/// ONE PLACE THAT KNOWS WHERE CREDENTIALS LIVE. Terminal, Exchange and Send all
/// need the same three files, and three copies of that knowledge would be three
/// things to change when the CA moves.
fn dials_for(
    model: &lab::Model,
    nodes: &[String],
    user: &str,
) -> Result<Vec<(String, ssh::Dial)>, String> {
    let home = match std::env::var("HOME") {
        Ok(h) => std::path::PathBuf::from(h).join(".copal"),
        Err(_) => std::path::PathBuf::from("/etc/copal"),
    };
    let mut out = Vec::new();
    for node in nodes {
        let address = model
            .nodes
            .iter()
            .find(|n| n.id == *node)
            .map(|n| n.address.clone())
            .unwrap_or_default();
        out.push((
            node.clone(),
            ssh::Dial::for_node(&home, &model.fleet, node, &address, user)?,
        ));
    }
    if out.is_empty() {
        return Err("nothing was selected".into());
    }
    Ok(out)
}

/// Open a node's desktop, on a thread.
///
/// THE CREDENTIALS ARE X.509 AND THE FLEET'S ARE NOT, YET. `ssh.rs` uses the
/// CA's OpenSSH certificates; TLS will not accept those, so this looks for an
/// X.509 pair beside them -- `<fleet>_ca.crt` for the node's certificate, and
/// `operator.crt`/`operator.key` for the client certificate the lockdown says
/// the node will demand. Issuing those is the CA's job and phase 10's work.
/// Until then this refuses with a sentence that names the missing file, which
/// is the same treatment every other missing credential gets.
fn open_desktop(
    fleet: String,
    node: String,
    address: String,
    user: String,
    out: Arc<Mutex<Option<Result<screen::Screen, String>>>>,
) {
    thread::spawn(move || {
        let home = match std::env::var("HOME") {
            Ok(h) => std::path::PathBuf::from(h).join(".copal"),
            Err(_) => std::path::PathBuf::from("/etc/copal"),
        };
        let result = (|| -> Result<screen::Screen, String> {
            let ca_path = home.join("ca").join(format!("{}_ca.crt", fleet));
            let ca_text = std::fs::read_to_string(&ca_path)
                .map_err(|e| format!("{}: {}", ca_path.display(), e))?;
            let cas: Vec<[u8; 32]> = x509::from_pem(&ca_text)?.iter().map(|c| c.key).collect();

            // The operator's own certificate, if the CA has issued one.
            let cert_path = home.join("fleets").join(&fleet).join("operator.crt");
            let key_path = home.join("fleets").join(&fleet).join("operator.key");
            let client = match (
                std::fs::read_to_string(&cert_path),
                std::fs::read_to_string(&key_path),
            ) {
                (Ok(c), Ok(k)) => Some((x509::from_pem(&c)?, x509::key_from_pem(&k)?)),
                _ => None,
            };

            let addr = if address.is_empty() {
                format!("{}.local:3389", node)
            } else if address.contains(':') {
                address.clone()
            } else {
                format!("{}:3389", address)
            };
            let session = rdp::Session::connect(&rdp::Dial {
                addr,
                host: node.clone(),
                user,
                domain: String::new(),
                cas,
                client,
                width: 1280,
                height: 720,
            })?;
            Ok(screen::Screen::open(node.clone(), session))
        })();
        *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
    });
}

/// Open a shell on a node, on a thread, and hand back the pane when it is up.
///
/// EVERY REFUSAL IN `ssh.rs` ARRIVES HERE AS A SENTENCE. A certificate from an
/// unknown CA, one that expired, one for another node, a node that cannot
/// prove it holds the key -- each is a string the operator can read, and none
/// of them is a prompt asking whether to continue anyway.
fn open_shell(
    fleet: String,
    node: String,
    address: String,
    user: String,
    out: Arc<Mutex<Option<Result<media::Job, String>>>>,
) {
    thread::spawn(move || {
        let home = match std::env::var("HOME") {
            Ok(h) => std::path::PathBuf::from(h).join(".copal"),
            Err(_) => std::path::PathBuf::from("/etc/copal"),
        };
        let job = ssh::Dial::for_node(&home, &fleet, &node, &address, &user)
            .and_then(|d| media::Job::remote(format!("{} — shell", node), &d, 100, 30));
        *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(job);
    });
}

/// Run a verb on a thread and post its per-node results.
///
/// `lab.rs` DRAWS; it does not shell out. The verb arrives here, where the
/// `Fleet` is, for the same reason every screen is a rendering of a command a
/// person could have typed.
fn run_verb(
    name: &'static str,
    nodes: Vec<String>,
    typed: Option<String>,
    fleet: Arc<Fleet>,
    results: Arc<Mutex<Option<lab::Results>>>,
) {
    // The bar's label and the allow-list's name are not the same string, and
    // the allow-list is the security boundary -- so the lookup happens here
    // and a label with no verb behind it runs nothing at all.
    let verb = match verbs::lookup(&name.to_ascii_lowercase()) {
        Some(v) => v,
        None => return,
    };
    // THE WORD THE OPERATOR TYPED IS VALIDATED HERE, not where it was typed.
    // `verbs.rs` is the allow-list and it owns what a scene name or a banner
    // may contain; the field is a place to type, not a place to decide.
    let arg = match verb.arg {
        verbs::Arg::None => None,
        _ => typed,
    };
    let plan = match verbs::build_argv(verb, arg.as_deref()) {
        Ok(p) => p,
        Err(why) => {
            *results.lock().unwrap_or_else(|e| e.into_inner()) = Some(lab::Results {
                verb: name.to_string(),
                running: false,
                items: vec![lab::Outcome {
                    node: nodes.first().cloned().unwrap_or_default(),
                    code: 1,
                    output: why.to_string(),
                }],
            });
            return;
        }
    };

    *results.lock().unwrap_or_else(|e| e.into_inner()) = Some(lab::Results {
        verb: name.to_string(),
        running: true,
        items: Vec::new(),
    });

    thread::spawn(move || {
        let out = fleet.verb_each(&plan, &nodes);
        let items = out
            .into_iter()
            .map(|r| lab::Outcome { node: r.node, code: r.code, output: r.output })
            .collect();
        *results.lock().unwrap_or_else(|e| e.into_inner()) = Some(lab::Results {
            verb: name.to_string(),
            running: false,
            items,
        });
    });
}

// Phase 2's specimen window has retired. `--gui` on Linux now opens the same
// interface `gui_loop` draws on a Mac, because `wl.rs` implements `Surface` --
// which is the whole return on phase 0 having happened first. The specimen
// itself is still reachable and still the regression test for `draw.rs`:
//
//     orrery --frame specimen.ppm --specimen
fn on_path(cmd: &str) -> bool {
    let p = std::path::Path::new(cmd);
    if p.is_absolute() || cmd.contains('/') {
        return p.is_file();
    }
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(cmd).is_file())
        })
        .unwrap_or(false)
}

fn usage() {
    eprintln!(
        "orrery -- the Copal fleet's web console

    --listen HOST:PORT   default 127.0.0.1:8080; the gallery screen wants
                         0.0.0.0 and invariant 6 wants a LAN
    --fleet NAME         passed to `copal fleet --fleet`
    --fleet-cmd CMD      default \"copal fleet\"
    --operator TOKEN     enable the write verbs; without it the console is
                         the read-mostly face §12 describes
    --demo               serve the lab report's museum from a fixture
    --quiet              do not log requests

  the seat -- one node, full screen, in this terminal

    --seat NODE          Observe and Control that node over VNC, instead of
                         serving the wall. No listener is opened.
    --vnc-port N         default 5900
    --fps N              frames to ask the node for, 1-60; default 6, because
                         frame rate is the axis III-B found expensive
  the native window (Wayland on a node, AppKit on a Mac)

    --gui                open a window instead of serving one: the room of
                         machines, the verbs, and the panes they open.
    --scale N            how many screen pixels one drawn pixel is, per side.
                         0 (the default) means whatever the panel needs -- the
                         Mac's backing scale factor, 1 on Wayland. The letters
                         are bitmaps, so this repeats pixels rather than
                         scaling glyphs: 2 is twice the size and just as crisp.
    --frame PATH         render one frame to a PPM and exit. Needs no
                         compositor, so it works on a headless node.
    --frame-view V       lab, media or files -- which face to render.
    --dark               the night palette, for either of those.
    --user NAME          the account on a node; copal-prep.sh's PI_USER.

  the seat, continued

    --sixel              force real pixels (iTerm2, kitty, foot, mlterm)
    --half-block         force the portable renderer, which is the default
                         anywhere sixel was not detected"
    );
}

fn serve(console: &Console, mut stream: TcpStream) {
    let req = match http::read_request(&stream) {
        Ok(r) => r,
        // A refusal still gets an answer; only a connection that said nothing
        // readable is dropped, because there is nobody on the other end to
        // read the sentence.
        Err(e) => {
            if !e.silent {
                if !console.quiet {
                    eprintln!("orrery: refused a request -> {} {}", e.code, e.msg);
                }
                let res = err(e.code, &e.msg);
                let _ = http::write_response(&mut stream, &res, false);
            }
            return;
        }
    };
    let head_only = req.method == "HEAD";
    let res = route(console, &req);
    if !console.quiet {
        eprintln!("orrery: {} {} -> {}", req.method, req.path, res.code);
    }
    let _ = http::write_response(&mut stream, &res, head_only);
}

fn route(console: &Console, req: &Request) -> Response {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET" | "HEAD", "/") => Response::html(page(console)),
        ("GET" | "HEAD", "/api/state") => match console.fleet.state() {
            Ok(doc) => Response::json(200, fleet::annotate(&doc)),
            Err(e) => Response::json(
                200,
                format!("{{\"error\":{}}}", json::Value::quote(&e)),
            ),
        },
        ("GET" | "HEAD", "/healthz") => Response::json(200, "{\"ok\":true}"),
        ("POST", "/api/verb") => verb(console, req),
        // The write route in the gallery posture is not 403 and not 405: it is
        // simply not there, and neither is anything else.
        _ => Response::json(404, "{\"error\":\"no such route\"}"),
    }
}

fn page(console: &Console) -> String {
    let name = console
        .fleet
        .state()
        .ok()
        .and_then(|d| {
            json::parse(&d)
                .ok()
                .and_then(|v| v.get("fleet").and_then(|f| f.as_str()).map(str::to_string))
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if console.fleet.fleet.is_empty() {
                "fleet".to_string()
            } else {
                console.fleet.fleet.clone()
            }
        });
    PAGE.replace("__FLEET__", &escape(&name))
        .replace(
            "__OPERATOR__",
            if console.operator.is_empty() { "false" } else { "true" },
        )
}

fn escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            '"' => "&quot;".chars().collect(),
            '\'' => "&#39;".chars().collect(),
            c => vec![c],
        })
        .collect()
}

/// Compare a token without letting the time taken say how much of it was right.
fn same_token(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn err(code: u16, msg: &str) -> Response {
    Response::json(
        code,
        format!("{{\"error\":{}}}", json::Value::quote(msg)),
    )
}

fn verb(console: &Console, req: &Request) -> Response {
    if console.operator.is_empty() {
        return err(404, "no such route");
    }
    match req.header("X-Copal-Token") {
        Some(t) if same_token(t, &console.operator) => {}
        _ => return err(401, "the operator token does not match"),
    }

    let body = match std::str::from_utf8(&req.body) {
        Ok(b) => b,
        Err(_) => return err(400, "unreadable request"),
    };
    let parsed = match json::parse(body) {
        Ok(v) => v,
        Err(_) => return err(400, "unreadable request"),
    };

    let name = match parsed.get("verb").and_then(|v| v.as_str()) {
        Some(v) => v,
        None => return err(400, "no verb named"),
    };
    let verb = match verbs::lookup(name) {
        Some(v) => v,
        None => return err(400, &format!("not a verb this console runs: {}", name)),
    };

    // The read model decides which names are real, so it is fetched before any
    // verb runs rather than trusted from the page.
    let doc_text = match console.fleet.state() {
        Ok(d) => d,
        Err(e) => return err(503, &format!("no read model, so no verb: {}", e)),
    };
    let doc = match json::parse(&doc_text) {
        Ok(d) => d,
        Err(e) => return err(503, &format!("no read model, so no verb: {}", e)),
    };

    let want: Vec<String> = parsed
        .get("nodes")
        .and_then(|n| n.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|n| n.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let nodes = match verbs::validate_nodes(&want, &doc) {
        Ok(n) => n,
        Err(e) => return err(400, &e.to_string()),
    };
    let arg = parsed.get("arg").and_then(|a| a.as_str());
    let plan = match verbs::build_argv(verb, arg.as_deref()) {
        Ok(p) => p,
        Err(e) => return err(400, &e.to_string()),
    };

    let results = console.fleet.verb_each(&plan, &nodes);
    // The worst code wins, because "I told eight machines to shut down" and
    // "eight machines shut down" are different claims and the second one is
    // only true when every result says so.
    let worst = results.iter().map(|r| r.code).max().unwrap_or(0);

    // A per-node column in text, which is what §IV-C asks Run to produce and
    // what the page renders as-is.
    let mut summary = String::new();
    for r in &results {
        summary.push_str(&format!(
            "{:<12} {}\n",
            r.node,
            if r.code == 0 { "ok".to_string() } else { format!("exit {}", r.code) }
        ));
        for line in r.output.lines() {
            summary.push_str(&format!("             {}\n", line));
        }
    }

    let rows: Vec<String> = results
        .iter()
        .map(|r| {
            format!(
                "{{\"node\":{},\"code\":{},\"output\":{}}}",
                json::Value::quote(&r.node),
                r.code,
                json::Value::quote(&r.output)
            )
        })
        .collect();

    let ran = if plan.per_node {
        format!("copal fleet {} --node <each of {}>", plan.argv.join(" "), nodes.len())
    } else {
        format!("copal fleet {}", plan.argv.join(" "))
    };

    Response::json(
        200,
        format!(
            "{{\"code\":{},\"output\":{},\"ran\":{},\"results\":[{}]}}",
            worst,
            json::Value::quote(summary.trim_end()),
            json::Value::quote(&ran),
            rows.join(",")
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console(operator: &str) -> Console {
        Console {
            fleet: Fleet::new(vec!["copal".into(), "fleet".into()], String::new(), true),
            operator: operator.to_string(),
            quiet: true,
        }
    }

    fn post(body: &str, token: Option<&str>) -> Request {
        Request {
            method: "POST".into(),
            path: "/api/verb".into(),
            headers: token
                .map(|t| vec![("X-Copal-Token".to_string(), t.to_string())])
                .unwrap_or_default(),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn the_gallery_posture_has_no_write_route() {
        let c = console("");
        let res = verb(&c, &post(r#"{"verb":"power","nodes":["museum-01"],"arg":"off"}"#, None));
        assert_eq!(res.code, 404, "the write route answered in the gallery posture");
        let body = String::from_utf8(res.body).unwrap();
        assert!(!body.contains("token"), "404 leaked that a token exists: {}", body);
    }

    #[test]
    fn a_write_needs_the_token() {
        let c = console("s3cret");
        assert_eq!(verb(&c, &post(r#"{"verb":"power","nodes":["museum-01"],"arg":"off"}"#, None)).code, 401);
        assert_eq!(verb(&c, &post(r#"{"verb":"power","nodes":["museum-01"],"arg":"off"}"#, Some("wrong"))).code, 401);
        assert_eq!(verb(&c, &post(r#"{"verb":"power","nodes":["museum-01"],"arg":"off"}"#, Some("s3cret"))).code, 200);
    }

    #[test]
    fn tokens_compare_without_leaking_their_length_in_the_answer() {
        assert!(same_token("abc", "abc"));
        assert!(!same_token("abc", "abd"));
        assert!(!same_token("abc", "ab"));
        assert!(!same_token("", "a"));
        assert!(same_token("", ""));
    }

    #[test]
    fn a_verb_that_runs_reports_what_it_ran() {
        let c = console("t");
        let res = verb(&c, &post(r#"{"verb":"scene","nodes":["museum-01","museum-03"],"arg":"show"}"#, Some("t")));
        assert_eq!(res.code, 200);
        let v = json::parse(&String::from_utf8(res.body).unwrap()).unwrap();
        // Two nodes is two runs, and the report says so per node.
        assert_eq!(
            v.get("ran").unwrap().as_str(),
            Some("copal fleet scene show --node <each of 2>")
        );
        let rows = v.get("results").unwrap().as_array().unwrap();
        assert_eq!(rows.len(), 2, "a two-node selection did not produce two results");
        assert_eq!(rows[0].get("node").unwrap().as_str(), Some("museum-01"));
        assert_eq!(rows[1].get("node").unwrap().as_str(), Some("museum-03"));
        for r in rows {
            let out = r.get("output").unwrap().as_str().unwrap();
            assert!(out.contains("--node"), "the node was not aimed at: {}", out);
        }
    }

    #[test]
    fn nothing_from_a_request_becomes_a_shell() {
        let c = console("t");
        for body in [
            r#"{"verb":"scene","nodes":["museum-01"],"arg":"show; rm -rf /"}"#,
            r#"{"verb":"scene","nodes":["../etc"],"arg":"show"}"#,
            r#"{"verb":"power","nodes":["museum-01"],"arg":"halt"}"#,
            r#"{"verb":"control","nodes":["museum-01"]}"#,
            r#"{"verb":"exec","nodes":["museum-01"],"arg":"id"}"#,
            r#"{"nodes":["museum-01"]}"#,
            r#"not json"#,
        ] {
            let res = verb(&c, &post(body, Some("t")));
            assert_eq!(res.code, 400, "accepted {}", body);
        }
    }

    #[test]
    fn the_page_carries_the_posture_and_nothing_off_the_lan() {
        let gallery = page(&console(""));
        assert!(gallery.contains("OPERATOR = false"));
        assert!(page(&console("t")).contains("OPERATOR = true"));
        // Invariant 6: nothing here reaches past the LAN.
        assert!(!gallery.contains("http://"), "the page reaches off the LAN");
        assert!(!gallery.contains("https://"), "the page reaches off the LAN");
        assert!(gallery.contains("<title>museum"), "the fleet name is missing");
    }

    /// The page's JavaScript must parse, and nothing else here proves it.
    ///
    /// The Python prototype's suite once passed 29 checks over a page whose
    /// script died on load -- a backtick inside a template literal ended the
    /// string, and every check was looking at the server rather than the page.
    /// If a JS engine is on the box, use it; if not, say so out loud rather
    /// than let a silent skip read as a pass.
    #[test]
    fn the_pages_javascript_parses() {
        let start = match PAGE.find("<script>") {
            Some(i) => i + "<script>".len(),
            None => panic!("the page has no script block"),
        };
        let end = PAGE[start..]
            .find("</script>")
            .expect("the script block is never closed")
            + start;
        let js = &PAGE[start..end];
        assert!(js.len() > 1000, "the script block is suspiciously short");

        let engine = ["node", "qjs", "d8"]
            .into_iter()
            .find(|e| super::on_path(e));
        let Some(engine) = engine else {
            eprintln!(
                "orrery: no node/qjs/d8 here -- the page's JavaScript was NOT parsed"
            );
            return;
        };

        let dir = std::env::temp_dir().join(format!("orrery-page-{}.js", std::process::id()));
        std::fs::write(&dir, js).expect("could not write the script out");
        let argv: Vec<&str> = if engine == "node" {
            vec!["--check"]
        } else {
            vec![]
        };
        let out = std::process::Command::new(engine)
            .args(&argv)
            .arg(&dir)
            .output()
            .expect("could not run the JS engine");
        let _ = std::fs::remove_file(&dir);
        assert!(
            out.status.success(),
            "the page's JavaScript does not parse:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn a_command_is_found_the_way_a_shell_would_find_it() {
        assert!(super::on_path("sh"), "sh was not found on PATH");
        assert!(!super::on_path("definitely-not-a-real-command-xyzzy"));
        assert!(super::on_path("/bin/sh"));
        assert!(!super::on_path("/bin/definitely-not-here-xyzzy"));
    }

    #[test]
    fn a_fleet_name_cannot_close_a_tag() {
        assert_eq!(escape("<script>"), "&lt;script&gt;");
        assert_eq!(escape("a&b\"c"), "a&amp;b&quot;c");
    }

    #[test]
    fn unknown_routes_are_all_the_same_answer() {
        let c = console("t");
        for (m, p) in [
            ("GET", "/../etc/passwd"),
            ("GET", "/api"),
            ("POST", "/"),
            ("DELETE", "/api/state"),
            ("GET", "/assets/wall.html"),
        ] {
            let req = Request { method: m.into(), path: p.into(), headers: vec![], body: vec![] };
            assert_eq!(route(&c, &req).code, 404, "{} {}", m, p);
        }
    }

    #[test]
    fn state_is_served_annotated() {
        let c = console("");
        let req = Request { method: "GET".into(), path: "/api/state".into(), headers: vec![], body: vec![] };
        let res = route(&c, &req);
        assert_eq!(res.code, 200);
        let v = json::parse(&String::from_utf8(res.body).unwrap()).unwrap();
        assert_eq!(v.get("_picture").unwrap().as_str(), Some("live"));
        assert_eq!(v.get("nodes").unwrap().as_array().unwrap().len(), 8);
    }
}

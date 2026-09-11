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

mod draw;
mod fleet;
mod font;
mod http;
mod json;
mod lab;
mod paint;
mod profile;
mod rfb;
mod seat;
mod surface;
mod ui;
mod verbs;
// The native GUI. Wayland is a Linux protocol and `sys.rs` declares Linux
// system calls, so both are absent elsewhere -- the seat and the wall still
// build on a developer's Mac, which is where most of this is written.
#[cfg(target_os = "linux")]
mod sys;
#[cfg(target_os = "linux")]
mod wl;

use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
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
    let mut specimen_frame = false;
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
            "--sixel" => { seat_opts.paint = Some(paint::Mode::Sixel); i += 1 }
            "--half-block" => { seat_opts.paint = Some(paint::Mode::HalfBlock); i += 1 }
            "--demo" => { demo = true; i += 1 }
            "--quiet" => { quiet = true; i += 1 }
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
    // --gui is exempt while it is phase 1: a window that paints a colour has
    // no read model to be missing. When the wall moves into it in phase 3 the
    // exemption comes back off, because then it will.
    if !demo && !gui && frame_to.is_empty() && !on_path(&cmd[0]) {
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
        #[cfg(target_os = "linux")]
        {
            if let Err(e) = gui_window() {
                eprintln!("orrery: {}", e);
                std::process::exit(1);
            }
            return;
        }
        #[cfg(not(target_os = "linux"))]
        {
            eprintln!(
                "orrery: --gui is Wayland, and Wayland is Linux. This binary was \n\
                 built for {}; build it on a node, or in a container, to use it.",
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
            Frame::Lab { ref doc, ref select, posture } => {
                let model = lab::Model::parse(doc);
                let mut state = lab::Lab::new(posture);
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

/// Phase 2's window: the specimen, redrawn whenever the compositor resizes it.
#[cfg(target_os = "linux")]
fn gui_window() -> Result<(), String> {
    use std::time::Duration;

    let mut win = wl::Window::open("orrery", 960, 600).map_err(|e| e.to_string())?;
    eprintln!("orrery: window open at {}x{}", win.width, win.height);

    let theme = draw::LIGHT;
    fn repaint(win: &mut wl::Window, theme: &draw::Theme) {
        let (w, h) = (win.width, win.height);
        let mut c = draw::Canvas::new(win.pixels(), w, h);
        specimen(&mut c, theme);
    }

    repaint(&mut win, &theme);
    win.present().map_err(|e| e.to_string())?;

    loop {
        let changes = win.poll().map_err(|e| e.to_string())?;
        if changes.closed {
            eprintln!("orrery: the window was closed");
            return Ok(());
        }
        if changes.resized {
            win.apply_resize().map_err(|e| e.to_string())?;
            eprintln!("orrery: resized to {}x{}", win.width, win.height);
            repaint(&mut win, &theme);
            win.present().map_err(|e| e.to_string())?;
        }
        // Nothing animates yet, so this is a poll rather than a frame clock.
        // `wl_surface.frame` is what phase 3 will want instead.
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// Is this a command we could actually run? An absolute path is checked
/// directly; a bare name is looked for along PATH, the way a shell would.
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
  the native window (Linux/Wayland)

    --gui                open a window instead of serving one. Phase 2: it
                         draws the specimen -- both faces, the palette and
                         every primitive the wall is made of.
    --frame PATH         render one frame to a PPM and exit. Needs no
                         compositor, so it works on a headless node.
    --dark               the night palette, for either of those.

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
    let plan = match verbs::build_argv(verb, arg) {
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

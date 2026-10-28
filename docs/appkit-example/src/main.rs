//! orbit: a Rust program with a Mac window and no crates.
//!
//!     orbit                 the window (Escape, Command-Q or the close button quits;
//!                           a click stops and starts the turning)
//!     orbit --frame F.ppm   one frame to a file, no window
//!
//! The loop is the program's: poll, change the state, draw, present, sleep.
//! AppKit is only asked for a window, events and a place to put pixels.

mod draw;
#[cfg(target_os = "macos")]
mod mac;
mod surface;

use surface::{Input, Offscreen, Surface};
use std::time::{Duration, Instant};

const FPS: u64 = 30;

/// The window's loop, written once against `Surface`.
fn run<S: Surface>(mut s: S) {
    let mut st = draw::State::default();
    let mut last = Instant::now();
    loop {
        for input in s.poll() {
            match input {
                Input::Closed | Input::Escape | Input::Key { ch: 'q', cmd: true } => return,
                Input::Moved { x, y } => st.pointer = Some((x, y)),
                Input::Clicked { x, y } => {
                    st.pointer = Some((x, y));
                    st.paused = !st.paused;
                }
                Input::Resized => s.apply_resize(),
                Input::Key { .. } => {}
            }
        }
        let now = Instant::now();
        if !st.paused {
            st.turned += (now - last).as_secs_f64();
        }
        last = now;
        let (w, h) = s.size();
        draw::frame(s.pixels(), w, h, &st);
        s.present();
        std::thread::sleep(Duration::from_millis(1000 / FPS));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--frame") {
        let path = args.get(i + 1).expect("--frame needs a file");
        let mut s = Offscreen::new(800, 500);
        let st = draw::State { pointer: Some((560, 140)), paused: false, turned: 3.0 };
        let (w, h) = s.size();
        draw::frame(s.pixels(), w, h, &st);
        s.write_ppm(path).expect("write the frame");
        return;
    }
    #[cfg(target_os = "macos")]
    run(mac::Window::open("orbit", 800, 500));
    #[cfg(not(target_os = "macos"))]
    {
        let _ = run::<Offscreen>;
        eprintln!("orbit: the window is macOS's; --frame works anywhere");
    }
}

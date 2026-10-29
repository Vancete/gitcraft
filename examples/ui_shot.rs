//! Headless screenshots of gitcraft driven by a small script, for checking the UI without a
//! window (and without touching the real mouse).
//!
//! `cargo run --example ui_shot -- <repo> <out-prefix> [step]...`
//!
//! Steps (coordinates in points, window is 1400×860):
//! `click:x,y`, `dclick:x,y`, `rclick:x,y`, `hover:x,y`, `drag:x1,y1,x2,y2`, `leave` (pointer off the window), `open:path` (a repository in a new tab), `key:Escape`, `ckey:P` (Ctrl/Cmd+key), `type:text`, `wait:ms`, `shot`.
//! Each `shot` writes `<out-prefix>-<n>.png`.

use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(repo), Some(out)) = (args.next(), args.next()) else {
        eprintln!(
            "usage: ui_shot <repo> <out-prefix> [click:x,y|rclick:x,y|hover:x,y|key:K|type:text|wait:ms|shot]..."
        );
        std::process::exit(2);
    };
    let steps: Vec<String> = args.collect();

    let mut harness = Harness::builder()
        .with_size([1400.0, 860.0])
        // A frame's worth of time per step, so two clicks make a double-click.
        .with_step_dt(1.0 / 60.0)
        .build_eframe(|cc| gitcraft::app::App::new(cc, Some(repo.into())));
    settle(&mut harness, 1500);

    let mut shots = 0;
    for step in steps {
        let (cmd, arg) = step.split_once(':').unwrap_or((step.as_str(), ""));
        match cmd {
            "click" | "rclick" | "cclick" | "sclick" => {
                let pos = point(arg);
                let button = if cmd == "rclick" {
                    PointerButton::Secondary
                } else {
                    PointerButton::Primary
                };
                let modifiers = match cmd {
                    "cclick" => Modifiers::COMMAND,
                    "sclick" => Modifiers::SHIFT,
                    _ => Modifiers::NONE,
                };
                harness.event(Event::PointerMoved(pos));
                harness.step();
                // The modifier held down around the click, as the keyboard reports it.
                harness.event(Event::ModifiersChanged(modifiers));
                harness.step();
                for pressed in [true, false] {
                    harness.event(Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers,
                    });
                }
                harness.step();
                harness.event(Event::ModifiersChanged(Modifiers::NONE));
                settle(&mut harness, 400);
            }
            "dclick" => {
                let pos = point(arg);
                harness.event(Event::PointerMoved(pos));
                harness.step();
                // Each press and release in a frame of its own, as a real double-click arrives.
                for _ in 0..2 {
                    for pressed in [true, false] {
                        harness.event(Event::PointerButton {
                            pos,
                            button: PointerButton::Primary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        });
                        harness.step();
                    }
                }
                settle(&mut harness, 600);
            }
            // `open:path`: another repository, in a new tab.
            "open" => {
                let ctx = harness.ctx.clone();
                harness
                    .state_mut()
                    .open_tab(std::path::Path::new(arg), &ctx);
                settle(&mut harness, 1500);
            }
            "leave" => {
                harness.remove_cursor();
                settle(&mut harness, 200);
            }
            // `wheel:x,y,dy`: the mouse wheel over a point; a negative `dy` scrolls down.
            "wheel" => {
                let n: Vec<f32> = arg
                    .split(',')
                    .map(|v| v.trim().parse().unwrap_or(0.0))
                    .collect();
                let (pos, dy) = (
                    Pos2::new(
                        n.first().copied().unwrap_or(0.0),
                        n.get(1).copied().unwrap_or(0.0),
                    ),
                    n.get(2).copied().unwrap_or(0.0),
                );
                harness.event(Event::PointerMoved(pos));
                harness.step();
                harness.event(Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, dy),
                    modifiers: Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                });
                settle(&mut harness, 600);
            }
            "drag" => {
                let n: Vec<f32> = arg
                    .split(',')
                    .map(|v| v.trim().parse().unwrap_or(0.0))
                    .collect();
                let (from, to) = match n[..] {
                    [a, b, c, d] => (Pos2::new(a, b), Pos2::new(c, d)),
                    _ => (Pos2::ZERO, Pos2::ZERO),
                };
                harness.event(Event::PointerMoved(from));
                harness.step();
                harness.event(Event::PointerButton {
                    pos: from,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                });
                harness.step();
                for k in 1..=8 {
                    harness.event(Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
                    harness.step();
                }
                harness.event(Event::PointerButton {
                    pos: to,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                });
                settle(&mut harness, 300);
            }
            "hover" => {
                harness.event(Event::PointerMoved(point(arg)));
                settle(&mut harness, 200);
            }
            "key" => {
                if let Some(key) = Key::from_name(arg) {
                    harness.key_press(key);
                }
                settle(&mut harness, 200);
            }
            "ckey" => {
                if let Some(key) = Key::from_name(arg) {
                    harness.key_press_modifiers(Modifiers::COMMAND, key);
                }
                settle(&mut harness, 300);
            }
            "type" => {
                harness.event(Event::Text(arg.to_owned()));
                settle(&mut harness, 200);
            }
            "wait" => settle(&mut harness, arg.parse().unwrap_or(500)),
            "shot" => {
                shots += 1;
                let path = format!("{out}-{shots}.png");
                match harness.render() {
                    Ok(image) => match image.save(&path) {
                        Ok(()) => println!("wrote {path}"),
                        Err(e) => eprintln!("{path}: {e}"),
                    },
                    Err(e) => eprintln!("render failed: {e}"),
                }
            }
            other => eprintln!("unknown step {other}"),
        }
    }
}

/// Steps frames for `ms` of real time, so background git work can land.
fn settle(harness: &mut Harness<'_, gitcraft::app::App>, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        harness.step();
        std::thread::sleep(Duration::from_millis(30));
    }
    harness.step();
}

fn point(arg: &str) -> Pos2 {
    let mut it = arg
        .split(',')
        .map(|n| n.trim().parse::<f32>().unwrap_or(0.0));
    Pos2::new(it.next().unwrap_or(0.0), it.next().unwrap_or(0.0))
}

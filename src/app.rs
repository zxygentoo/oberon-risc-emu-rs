//! The application: machine + device wiring shared by both frontends, the
//! windowed app (window, 60 fps clock loop, render, and input dispatch — port
//! of `sdl-main.c`), and the `--headless` runner.

use std::num::NonZeroU32;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::window::{Fullscreen, Window, WindowId};

use risc_core::clipboard::ClipboardBridge;
use risc_core::disk::Disk;
use risc_core::headless::{CPU_HZ, FPS};
use risc_core::io::Led;
use risc_core::pclink::PcLink;
use risc_core::risc::Risc;

use oberon_risc_emu::render::{self, BLACK, WHITE};

use crate::cli;
use crate::clipboard::ArboardClipboard;
use crate::error::Result;
use crate::input;

type SbContext = softbuffer::Context<Rc<Window>>;
type SbSurface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

/// Parse args, build the core + devices, and run the GUI (or, with
/// `--headless`, the windowless frame loop). Entry point for the `risc` binary.
pub fn run() -> Result<()> {
    use clap::Parser;
    let cfg = cli::Cli::parse().into_config()?;
    if cfg.headless {
        return run_headless(&cfg);
    }

    let risc = build_machine(&cfg, cfg.disk_image.as_deref())?;
    let event_loop = EventLoop::new()?;
    let mut app = App::new(risc, cfg);
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Build the core and wire its devices from the validated `Config`, with
/// `disk_path` as the SPI disk — shared by the windowed and headless paths so
/// every option composes with both.
fn build_machine(cfg: &cli::Config, disk_path: Option<&Path>) -> Result<Box<Risc>> {
    let mut risc = Box::new(Risc::new());

    // Default devices, as the C frontend wires them: PCLink serial + clipboard
    // (arboard degrades to a no-op where no host clipboard is reachable).
    risc.set_serial(Box::new(PcLink::new()));
    risc.set_clipboard(Box::new(ClipboardBridge::new(Box::new(
        ArboardClipboard::new(),
    ))));

    if cfg.configure {
        risc.configure_memory(cfg.mem, cfg.width as i32, cfg.height as i32);
    }
    if cfg.boot_from_serial {
        risc.set_switches(1);
    }
    if cfg.leds {
        risc.set_leds(Box::new(LedLogger));
    }

    // The disk is the SPI slave at index 1; a diskless card allows
    // --boot-from-serial.
    risc.set_spi(1, Box::new(Disk::new(disk_path)?));

    // --serial-in/--serial-out replace PCLink with a raw host serial line.
    if cfg.serial_in.is_some() || cfg.serial_out.is_some() {
        #[cfg(unix)]
        {
            let in_path = cfg.serial_in.as_deref().unwrap_or("/dev/null");
            let out_path = cfg.serial_out.as_deref().unwrap_or("/dev/null");
            let serial =
                risc_core::raw_serial::RawSerial::new(Path::new(in_path), Path::new(out_path))?;
            risc.set_serial(Box::new(serial));
        }
        #[cfg(not(unix))]
        {
            return Err(crate::error::Error::Config(
                "--serial-in/--serial-out are only supported on unix".into(),
            ));
        }
    }

    Ok(risc)
}

/// Run without a window. With `--frames` it's a deterministic experiment for CI
/// checks and golden-hash regeneration: boot a throwaway copy of the image for
/// exactly N synthetic-clock frames, then print FNV-1a hashes and a blank-word
/// count. Without, it's a headless session (e.g. over ssh, talked to via
/// `--serial-in`/`--serial-out`): write through to the image like the windowed
/// emulator and pace the same 60 Hz clock by wall time until killed.
fn run_headless(cfg: &cli::Config) -> Result<()> {
    let Some(frames) = cfg.frames else {
        let mut risc = build_machine(cfg, cfg.disk_image.as_deref())?;
        let period = Duration::from_nanos(1_000_000_000 / FPS as u64);
        let start = Instant::now();
        let mut next_frame = start;
        loop {
            let now = Instant::now();
            if now < next_frame {
                std::thread::sleep(next_frame - now);
            }
            risc.set_time(start.elapsed().as_millis() as u32);
            risc.run(CPU_HZ / FPS);
            next_frame += period;
            if next_frame < Instant::now() {
                // Fell behind (e.g. after a stall); resync rather than spiral.
                next_frame = Instant::now() + period;
            }
        }
    };

    // Boot writes to the disk, so run against a throwaway copy: the source image
    // is left untouched and repeated runs stay reproducible.
    let scratch = match &cfg.disk_image {
        Some(src) => {
            let mut tmp = std::env::temp_dir();
            tmp.push(format!("oberon_headless_{}.dsk", std::process::id()));
            std::fs::copy(src, &tmp)?;
            Some(tmp)
        }
        None => None, // --boot-from-serial, diskless
    };
    let mut risc = build_machine(cfg, scratch.as_deref())?;
    risc_core::headless::run_frames(&mut risc, frames);

    // The hashes are the machine-checkable result; the blank-word count is the
    // human-readable liveness signal (a booted screen is mostly non-blank).
    let words = (risc.fb_width() * risc.fb_height()) as usize;
    let blank = risc.framebuffer()[..words]
        .iter()
        .filter(|&&w| w == 0)
        .count();
    println!(
        "frames={frames} framebuffer_fnv1a=0x{:016x} state_fnv1a=0x{:016x} blank_words={blank}/{words}",
        risc_core::headless::framebuffer_hash(&risc),
        risc_core::headless::state_hash(&risc),
    );

    if let Some(tmp) = scratch {
        let _ = std::fs::remove_file(tmp);
    }
    Ok(())
}

/// The LED device for `--leds`: logs the 8-bit state to stdout (port of `show_leds`).
struct LedLogger;
impl Led for LedLogger {
    fn write(&mut self, value: u32) {
        let mut s = String::from("LEDs: ");
        for i in (0..8).rev() {
            s.push(if value & (1 << i) != 0 {
                (b'0' + i) as char
            } else {
                '-'
            });
        }
        println!("{s}");
    }
}

pub(crate) struct App {
    // Fields marked `pub(crate)` are the ones the sibling `input` module reaches
    // into; the rest stay private to this module.
    pub(crate) risc: Box<Risc>,
    cfg: cli::Config,

    pub(crate) tex_w: usize,
    pub(crate) tex_h: usize,
    texture: Vec<u32>,

    // Persistent window-sized scaled image. Each frame only the damaged span is
    // re-scaled into it (the rest carries over), then the whole buffer is copied
    // to the surface — softbuffer's surface buffer isn't guaranteed to persist
    // between presents. `full_repaint` forces a whole-window rescale (initial +
    // resize, when the scale factor changes).
    scaled: Vec<u32>,
    full_repaint: bool,

    pub(crate) window: Option<Rc<Window>>,
    // Held for the surface's display connection; never read after construction.
    #[allow(dead_code)]
    context: Option<SbContext>,
    surface: Option<SbSurface>,

    win_w: u32,
    win_h: u32,
    pub(crate) rect: render::DisplayRect,
    fullscreen: bool,

    pub(crate) modifiers: ModifiersState,
    pub(crate) mouse_offscreen: bool,

    start: Instant,
    next_frame: Instant,
}

impl App {
    fn new(risc: Box<Risc>, cfg: cli::Config) -> Self {
        let tex_w = (risc.fb_width() * 32) as usize;
        let tex_h = risc.fb_height() as usize;
        let now = Instant::now();
        App {
            fullscreen: cfg.fullscreen,
            tex_w,
            tex_h,
            texture: vec![BLACK; tex_w * tex_h],
            scaled: vec![BLACK; tex_w * tex_h],
            full_repaint: true,
            risc,
            cfg,
            window: None,
            context: None,
            surface: None,
            win_w: tex_w as u32,
            win_h: tex_h as u32,
            rect: render::DisplayRect {
                x: 0,
                y: 0,
                w: tex_w as i32,
                h: tex_h as i32,
                scale: 1.0,
            },
            modifiers: ModifiersState::empty(),
            mouse_offscreen: false,
            start: now,
            next_frame: now,
        }
    }

    fn reconfigure(&mut self, w: u32, h: u32) {
        self.win_w = w;
        self.win_h = h;
        self.rect = render::scale_display(w, h, self.tex_w as u32, self.tex_h as u32);
        // The scale factor changed, so the whole window must be re-scaled.
        self.scaled = vec![BLACK; (w * h) as usize];
        self.full_repaint = true;
    }

    fn render(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if self.surface.is_none() {
            return;
        }

        // Refresh the native texture from framebuffer damage, then re-scale only
        // the affected window span into the persistent buffer (everything, on the
        // first frame or after a resize).
        let dirty = render::blit_damage(&mut self.texture, &mut self.risc, BLACK, WHITE);
        if self.full_repaint {
            render::scale_into(
                &mut self.scaled,
                self.win_w,
                self.win_h,
                &self.texture,
                self.tex_w,
                self.tex_h,
                self.rect,
            );
            self.full_repaint = false;
        } else if let Some(tex_rect) = dirty {
            let wd = render::window_dirty(tex_rect, self.rect, self.win_w, self.win_h);
            render::scale_region(
                &mut self.scaled,
                self.win_w,
                self.win_h,
                &self.texture,
                self.tex_w,
                self.tex_h,
                self.rect,
                wd,
            );
        }

        // Copy the persistent image into the surface buffer (its prior contents
        // are not guaranteed) and present.
        let (Some(nw), Some(nh)) = (NonZeroU32::new(self.win_w), NonZeroU32::new(self.win_h))
        else {
            return;
        };
        let surface = self.surface.as_mut().unwrap();
        if surface.resize(nw, nh).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else {
            return;
        };
        // A fresh surface buffer's contents are unspecified; never present one
        // we didn't fully paint. (The sizes always agree today — both derive
        // from win_w * win_h — so this is a guard, not a code path.)
        if buf.len() != self.scaled.len() {
            return;
        }
        buf.copy_from_slice(&self.scaled);
        window.pre_present_notify();
        let _ = buf.present();
    }

    pub(crate) fn toggle_fullscreen(&mut self) {
        self.fullscreen = !self.fullscreen;
        if let Some(window) = &self.window {
            window.set_fullscreen(self.fullscreen.then(|| Fullscreen::Borderless(None)));
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let zoom = if self.cfg.zoom > 0.0 {
            self.cfg.zoom
        } else {
            // Auto: 2x if a monitor is at least twice the framebuffer in both
            // dimensions, else 1x (mirrors the C's display-bounds check).
            let big = event_loop
                .primary_monitor()
                .or_else(|| event_loop.available_monitors().next())
                .is_some_and(|m| {
                    let s = m.size();
                    s.width >= self.tex_w as u32 * 2 && s.height >= self.tex_h as u32 * 2
                });
            if big {
                2.0
            } else {
                1.0
            }
        };

        let w = (self.tex_w as f64 * zoom).round() as u32;
        let h = (self.tex_h as f64 * zoom).round() as u32;
        let attrs = Window::default_attributes()
            .with_title("Project Oberon")
            .with_inner_size(PhysicalSize::new(w, h));
        let window = match event_loop.create_window(attrs) {
            Ok(win) => Rc::new(win),
            Err(e) => {
                eprintln!("could not create window: {e}");
                event_loop.exit();
                return;
            }
        };
        window.set_cursor_visible(false);
        if self.fullscreen {
            window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        }

        let context = match SbContext::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("could not create softbuffer context: {e}");
                event_loop.exit();
                return;
            }
        };
        let surface = match SbSurface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("could not create softbuffer surface: {e}");
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
        self.reconfigure(size.width.max(1), size.height.max(1));

        let now = Instant::now();
        self.start = now;
        self.next_frame = now;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.reconfigure(size.width.max(1), size.height.max(1)),
            WindowEvent::RedrawRequested => self.render(),
            other => input::handle(self, event_loop, other),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            return;
        }
        let now = Instant::now();
        if now >= self.next_frame {
            let ms = now.duration_since(self.start).as_millis() as u32;
            self.risc.set_time(ms);
            self.risc.run(CPU_HZ / FPS);
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            let period = Duration::from_nanos(1_000_000_000 / FPS as u64);
            self.next_frame += period;
            if self.next_frame < now {
                // Fell behind (e.g. after a stall); resync rather than spiral.
                self.next_frame = now + period;
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }
}

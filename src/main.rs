//! open-scrantic: a modern reimplementation of Screen Antics: Johnny Castaway (Sierra/Dynamix, 1992).
//! Runs on the original, installed screensaver files (SCRANTIC.SCR, RESOURCE.MAP, RESOURCE.001).

// The screensaver build is a GUI program: Windows must not open a console window behind it.
#![cfg_attr(all(windows, feature = "screensaver"), windows_subsystem = "windows")]
#![cfg_attr(not(feature = "screensaver"), forbid(unsafe_code))]

#[cfg(not(feature = "screensaver"))]
mod cli;
#[cfg(feature = "screensaver")]
mod saver;

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};

use open_scrantic::engine::config::Config;
use open_scrantic::engine::gfx::{HEIGHT, WIDTH};
use open_scrantic::engine::{Clock, Engine, STEP_MS, TICK_MS};
use open_scrantic::overlay::Overlay;
use open_scrantic::present;
use open_scrantic::sea::{self, Bands, FarIsland, Layout, Screen, Surroundings};
#[cfg(feature = "screensaver")]
use open_scrantic::windows_ui;
use open_scrantic::{assets, audio};

/// The title screen stays up while the 1992 machine loaded the first session.
const INTRO_HOLD: Duration = Duration::from_secs(2);

/// Raised by SIGTERM, SIGINT or SIGHUP (Unix): the run then ends like Esc ends it, saving the story progress, instead
/// of the process dying with it unsaved. That is how a desktop's idle daemon stops a screensaver.
fn termination_flag() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ] {
        if let Err(e) = signal_hook::flag::register(signal, Arc::clone(&flag)) {
            eprintln!("warning: cannot handle signal {signal}: {e}");
        }
    }
    flag
}

/// Where the picture is shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Surface {
    /// A window the user can move and resize; it closes with Esc.
    Window,
    /// The whole screen, without a cursor.
    Fullscreen,
    /// The preview pane of the Screen Saver Settings window (the handle Windows passes after `/p`): a child
    /// window that fills the pane.
    #[cfg(feature = "screensaver")]
    Embedded(isize),
}

/// A monitor of the layout to cover, with winit's handle to go fullscreen on (if winit knows it).
type Placement = (usize, Option<MonitorHandle>);

/// A window and what it shows.
struct View {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    /// The monitor of [`App::layout`] it covers; `None` for the only window (no sea around the picture).
    screen: Option<usize>,
    /// The monitor to go fullscreen on once more after the first frame: some compositors (Hyprland) map a second
    /// window that asked for it before being shown as an ordinary one.
    refullscreen: Option<Fullscreen>,
}

impl View {
    fn shows_picture(&self, layout: Option<&Layout>) -> bool {
        match (self.screen, layout) {
            (Some(i), Some(l)) => i == l.main,
            _ => true,
        }
    }
}

/// Not in the original: how to fill all monitors (`--all-monitors`, or AllMonitors of the INI).
struct AllMonitors {
    /// The monitors as the desktop places them, if given (Linux: winit cannot tell positions under Wayland);
    /// otherwise winit's.
    screens: Option<Vec<Screen>>,
    /// The name of the monitor for the picture; otherwise the primary one.
    main: Option<String>,
}

struct App {
    engine: Engine,
    audio: Option<audio::Audio>,
    views: Vec<View>,
    /// Picture on one monitor and the sea on the others, if wanted.
    all_monitors: Option<AllMonitors>,
    layout: Option<Layout>,
    /// What the sea shows around the picture, for the session numbered first.
    around: Option<(u64, Surroundings)>,
    /// The sea of the current step as RGB, made once for all the windows that show it.
    sea_rgb: Option<(u64, Vec<u32>)>,
    start: Instant,
    steps: u64,
    ticks: u64,
    /// Where the picture is shown.
    target: Surface,
    /// Ends the run at the first sign of the user (screensaver build only; a preview has none).
    #[cfg(feature = "screensaver")]
    input: Option<saver::InputGuard>,
    dedither: bool,
    /// Clock and weather over the picture (this fork's addition), if enabled.
    overlay: Option<Overlay>,
    /// Settings as they are in the INI, which a run with overrides must write back unchanged.
    saved: Config,
    /// F12 was pressed; the next redraw saves the frame it draws.
    screenshot: bool,
    /// The title screen has been requested once; until the first step it needs no further redraws.
    intro_drawn: bool,
    /// Set when the process was asked to terminate (see [`termination_flag`]).
    terminate: Arc<AtomicBool>,
    /// The Wayland app id of the windows (`--app-id`), for the compositor's window rules.
    app_id: Option<String>,
}

impl App {
    /// Advances the virtual Windows 3.1 clocks: a story step every 55 ms and the 16 ms tick.
    fn advance(&mut self) -> bool {
        let elapsed = self.start.elapsed().as_millis() as u64;
        let due = elapsed / STEP_MS;
        let mut stepped = false;
        // After long stalls (sleep, debugger) resume instead of fast-forwarding.
        if due > self.steps + 20 {
            self.steps = due - 1;
            self.ticks = self.steps * STEP_MS / TICK_MS;
        }
        while self.steps < due {
            self.steps += 1;
            let virtual_ms = self.steps * STEP_MS;
            while (self.ticks + 1) * TICK_MS <= virtual_ms {
                self.ticks += 1;
                self.engine.tick();
            }
            self.engine.step(&Clock::now());
            for id in self.engine.take_sounds() {
                if let Some(a) = self.audio.as_mut().filter(|_| self.engine.config.sounds) {
                    a.play(self.engine.sound_bank(), id);
                }
            }
            stepped = true;
        }
        stepped
    }

    /// Writes the current frame next to the executable: `-raw` is the 640x480 palette picture as the
    /// engine drew it, `-view` is what the window shows (dedithering included) before scaling.
    fn save_screenshot(&self) {
        let c = Clock::now();
        let name = format!(
            "screenshot-{:04}{:02}{:02}-{:02}{:02}-{}",
            c.year, c.month, c.day, c.hour, c.minute, self.steps
        );
        let px = &self.engine.frame().px;
        let pal = &self.engine.palette;
        let view = if self.dedither {
            present::dedither(px, pal)
        } else {
            present::plain(px, pal)
        };
        for (suffix, frame) in [("raw", present::plain(px, pal)), ("view", view)] {
            let path = open_scrantic::app_dir().join(format!("{name}-{suffix}.png"));
            let png = present::png(WIDTH as usize, HEIGHT as usize, &frame);
            match std::fs::write(&path, png) {
                Ok(()) => eprintln!("saved {}", path.display()),
                Err(e) => eprintln!("warning: cannot save {}: {e}", path.display()),
            }
        }
    }

    fn rgb(&self, px: &[u8]) -> Vec<u32> {
        if self.dedither {
            present::dedither(px, &self.engine.palette)
        } else {
            present::plain(px, &self.engine.palette)
        }
    }

    /// Makes the sea of this step ready for the windows that show it: its RGB frame and the far island.
    fn prepare_sea(&mut self) {
        let Some(layout) = &self.layout else { return };
        if self.sea_rgb.as_ref().is_none_or(|(step, _)| *step != self.steps)
            && let Some(frame) = self.engine.sea_frame()
        {
            self.sea_rgb = Some((self.steps, self.rgb(&frame.px)));
        }
        if self.around.as_ref().is_none_or(|(n, _)| *n != self.engine.sessions)
            && let (Some((bg, horizon)), Some(island)) = (self.engine.sea_background(), self.engine.far_island())
        {
            let pal = &self.engine.palette;
            let far = match layout.outermost() {
                Some(screen) if horizon > 0 => FarIsland::new(layout, screen, horizon, island, pal),
                _ => None,
            };
            let bands = Bands::new(&bg.px, pal, horizon);
            let still = self.rgb(&bg.px);
            self.around = Some((self.engine.sessions, Surroundings { bands, far, still }));
        }
    }

    fn redraw(&mut self, id: WindowId) {
        if std::mem::take(&mut self.screenshot) {
            self.save_screenshot();
        }
        let Some(i) = self.views.iter().position(|v| v.window.id() == id) else {
            return;
        };
        let picture = self.views[i].shows_picture(self.layout.as_ref());
        let rgb = picture.then(|| self.rgb(&self.engine.frame().px));
        self.prepare_sea();
        let view = &mut self.views[i];
        if let Some(fullscreen) = view.refullscreen.take() {
            view.window.set_fullscreen(Some(fullscreen));
        }
        let size = view.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if view.surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = view.surface.buffer_mut() else { return };
        let (w, h) = (size.width as usize, size.height as usize);
        let mut keep = None;
        if let Some(rgb) = &rgb {
            present::scale(rgb, &mut buf, w, h);
            keep = Some(present::placement(w, h));
        }
        if let (Some(layout), Some(screen), Some((_, sea_rgb)), Some((_, around))) =
            (&self.layout, view.screen, &self.sea_rgb, &self.around)
        {
            sea::draw(&mut buf, w, h, (layout, screen), sea_rgb, around, keep);
        }
        if picture && let Some(overlay) = &mut self.overlay {
            overlay.draw(&mut buf, w, h);
        }
        let _ = buf.present();
    }
}

/// Moves the window so its whole frame (title bar included) is centred on the primary monitor.
fn center_on_primary_monitor(window: &Window, event_loop: &ActiveEventLoop) {
    let Some(monitor) = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next())
    else {
        return;
    };
    let (m, origin) = (monitor.size(), monitor.position());
    let w = window.outer_size();
    let x = origin.x + (m.width as i32 - w.width as i32) / 2;
    let y = origin.y + (m.height as i32 - w.height as i32) / 2;
    window.set_outer_position(PhysicalPosition::new(x, y));
}

impl App {
    /// The monitors to cover, the one for the picture last, each with winit's handle to go fullscreen on (if
    /// winit knows it), and their layout. `None` when winit lists no monitor.
    fn plan_monitors(all: &AllMonitors, event_loop: &ActiveEventLoop) -> Option<(Layout, Vec<Placement>)> {
        let handles: Vec<MonitorHandle> = event_loop.available_monitors().collect();
        let screens = all.screens.clone().unwrap_or_else(|| {
            handles
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let (p, s) = (m.position(), m.size());
                    Screen {
                        name: m.name().unwrap_or_else(|| format!("#{i}")),
                        x: p.x as f64,
                        y: p.y as f64,
                        w: s.width as f64,
                        h: s.height as f64,
                    }
                })
                .collect()
        });
        if screens.is_empty() {
            return None;
        }
        let primary = event_loop.primary_monitor().and_then(|m| m.name());
        let main_name = all.main.clone().or(primary);
        let main = main_name
            .and_then(|n| screens.iter().position(|s| s.name == n))
            .unwrap_or(0);
        let handle = |name: &str| handles.iter().find(|m| m.name().as_deref() == Some(name)).cloned();
        // The picture's window last: Hyprland makes a window that maps on a monitor other than the focused one
        // fullscreen only while it is the newest, and the last one keeps the focus (and the keyboard).
        let mut out = Vec::new();
        for (i, s) in screens.iter().enumerate() {
            if i != main {
                match handle(&s.name) {
                    Some(h) => out.push((i, Some(h))),
                    None => eprintln!("warning: no monitor named {:?} to show the sea on", s.name),
                }
            }
        }
        out.push((main, handle(&screens[main].name)));
        Some((Layout { screens, main }, out))
    }

    /// Title and app id of every window.
    fn base_attributes(&self) -> WindowAttributes {
        #[allow(unused_mut)]
        let mut attrs = Window::default_attributes().with_title("Open Scrantic");
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(id) = &self.app_id {
            use winit::platform::wayland::WindowAttributesExtWayland;
            attrs = attrs.with_name(id.clone(), "");
        }
        attrs
    }

    fn window_attributes(&self, event_loop: &ActiveEventLoop) -> WindowAttributes {
        let attrs = self.base_attributes();
        let min = PhysicalSize::new(WIDTH as u32, HEIGHT as u32);
        match self.target {
            Surface::Fullscreen => attrs
                .with_min_inner_size(min)
                .with_fullscreen(Some(Fullscreen::Borderless(None))),
            Surface::Window => {
                // Largest whole multiple of the original 640x480 that fills at most 80% of the screen.
                let k = event_loop
                    .primary_monitor()
                    .or_else(|| event_loop.available_monitors().next())
                    .map_or(1, |m| {
                        let s = m.size();
                        (s.width * 4 / 5 / WIDTH as u32)
                            .min(s.height * 4 / 5 / HEIGHT as u32)
                            .max(1)
                    });
                attrs
                    .with_min_inner_size(min)
                    .with_inner_size(PhysicalSize::new(WIDTH as u32 * k, HEIGHT as u32 * k))
            }
            #[cfg(feature = "screensaver")]
            Surface::Embedded(parent) => windows_ui::preview_attributes(parent),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.views.is_empty() {
            return;
        }
        let planned = match (&self.all_monitors, self.target) {
            (Some(all), Surface::Fullscreen) => Self::plan_monitors(all, event_loop),
            _ => None,
        };
        let monitors = match planned {
            Some((layout, monitors)) => {
                self.layout = Some(layout);
                monitors
            }
            None => Vec::new(),
        };
        let wanted: Vec<(WindowAttributes, Option<usize>, Option<Fullscreen>)> = if monitors.is_empty() {
            vec![(self.window_attributes(event_loop), None, None)]
        } else {
            monitors
                .into_iter()
                .map(|(screen, handle)| {
                    let fullscreen = Fullscreen::Borderless(handle);
                    let attrs = self.base_attributes().with_fullscreen(Some(fullscreen.clone()));
                    (attrs, Some(screen), Some(fullscreen))
                })
                .collect()
        };
        for (attrs, screen, refullscreen) in wanted {
            let window = match event_loop.create_window(attrs) {
                Ok(w) => Rc::new(w),
                Err(e) => {
                    eprintln!("error: cannot create window: {e}");
                    event_loop.exit();
                    return;
                }
            };
            if self.target == Surface::Window {
                center_on_primary_monitor(&window, event_loop);
            }
            window.set_cursor_visible(self.target == Surface::Window);
            let surface = softbuffer::Context::new(Rc::clone(&window))
                .and_then(|ctx| softbuffer::Surface::new(&ctx, Rc::clone(&window)));
            match surface {
                Ok(surface) => self.views.push(View {
                    window,
                    surface,
                    screen,
                    refullscreen,
                }),
                Err(e) => {
                    eprintln!("error: cannot create drawing surface: {e}");
                    event_loop.exit();
                    return;
                }
            }
        }
        self.start = Instant::now() + INTRO_HOLD;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        #[cfg(feature = "screensaver")]
        if self.input.as_mut().is_some_and(|input| input.ends_run(&event)) {
            event_loop.exit();
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(id),
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape) =>
            {
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::F12) =>
            {
                self.screenshot = true;
                if let Some(v) = self.views.iter().find(|v| v.shows_picture(self.layout.as_ref())) {
                    v.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.terminate.load(Ordering::Relaxed) {
            event_loop.exit();
            return;
        }
        if self.views.is_empty() {
            return;
        }
        // The preview ends with the settings window it draws into.
        #[cfg(feature = "screensaver")]
        if let Surface::Embedded(parent) = self.target
            && !windows_ui::window_exists(parent)
        {
            event_loop.exit();
            return;
        }
        let stepped = self.advance();
        if stepped || !self.intro_drawn {
            self.intro_drawn = true;
            for v in &self.views {
                v.window.request_redraw();
            }
        }
        let next = self.start + Duration::from_millis((self.steps + 1) * STEP_MS);
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // A preview must not move the story on: only real runs save their progress.
        #[cfg(feature = "screensaver")]
        if matches!(self.target, Surface::Embedded(_)) {
            return;
        }
        let mut cfg = self.engine.config.clone();
        (cfg.background, cfg.clouds, cfg.waves, cfg.sounds) = (
            self.saved.background,
            self.saved.clouds,
            self.saved.waves,
            self.saved.sounds,
        );
        if let Err(e) = cfg.save() {
            eprintln!("warning: cannot save settings: {e}");
        }
    }
}

fn main() {
    #[cfg(not(feature = "screensaver"))]
    let opts = match cli::parse_args() {
        Ok(Some(o)) => o,
        Ok(None) => return,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    #[cfg(feature = "screensaver")]
    let Some(opts) = saver::start() else { return };
    let data = assets::ScreensaverData::load_first(&open_scrantic::data_dirs()).unwrap_or_else(|e| opts.fail_data(e));
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let saved = Config::load();
    let mut engine = Engine::new(data, opts.config(&saved), seed).unwrap_or_else(|e| opts.fail(e));
    let all_monitors = opts.all_monitors(&saved);
    if all_monitors.is_some() {
        engine.enable_sea(seed).unwrap_or_else(|e| opts.fail(e));
    }
    let audio = opts.audio(&saved);
    let dedither = opts.dedither(&saved);
    let overlay = opts.overlay(&saved);
    let event_loop = EventLoop::new().unwrap_or_else(|e| opts.fail(e));
    let mut app = App {
        engine,
        audio,
        views: Vec::new(),
        all_monitors: all_monitors.map(|(screens, main)| AllMonitors { screens, main }),
        layout: None,
        around: None,
        sea_rgb: None,
        start: Instant::now(),
        steps: 0,
        ticks: 0,
        target: opts.surface(),
        #[cfg(feature = "screensaver")]
        input: opts.input_guard(),
        dedither,
        overlay,
        saved,
        screenshot: false,
        intro_drawn: false,
        terminate: termination_flag(),
        #[cfg(not(feature = "screensaver"))]
        app_id: opts.app_id(),
        #[cfg(feature = "screensaver")]
        app_id: None,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("error: {e}");
    }
}

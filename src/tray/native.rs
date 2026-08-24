//! macOS tray icon via `tray-icon`, driven by a `winit` event loop that
//! owns the process's actual main thread -- required by macOS's native
//! GUI plumbing (a Cocoa run loop specifically has to pump on the main
//! thread; see the module-level note in `tray/mod.rs`). Windows has its
//! own native window instead (`window/windows.rs`); this tray-icon-based
//! `winit` event loop would work there too in principle, but there's no
//! reason to keep it once a platform has a real native window.
//!
//! Because the event loop owns this thread, the pairing server instead
//! runs on a background Tokio runtime: it's started with one `block_on`
//! call before the event loop starts (so the dashboard URL is known up
//! front), and its worker threads then keep driving already-spawned
//! connections independently of this thread until `run_app` returns.

use std::sync::Arc;

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use super::TrayCallbacks;
use crate::injector::Injector;
use crate::server::PairingServer;

/// Events routed onto the winit event loop from elsewhere: the global
/// muda/tray-icon callback handlers (which fire on an OS-owned thread) and
/// the `quit` tray-menu action, which must reach `ActiveEventLoop::exit`
/// and so can't just be an arbitrary `Fn()` like the other two callbacks.
enum UserEvent {
    MenuClicked(MenuId),
    TrayClicked,
    Quit,
}

/// Builds a small solid circle as the tray icon. Generated in-process
/// (rather than shipping an image asset) since it only needs to be
/// recognizable, not branded.
fn build_icon() -> Icon {
    const SIZE: u32 = 32;
    let center = (SIZE - 1) as f32 / 2.0;
    let radius = SIZE as f32 / 2.0 - 1.0;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            if (dx * dx + dy * dy).sqrt() <= radius {
                rgba.extend_from_slice(&[0x2f, 0x6f, 0xeb, 0xff]); // opaque accent blue
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]); // transparent
            }
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE)
        .expect("procedurally generated icon dimensions are always valid")
}

fn build_tray_icon(open_id: &MenuId, regenerate_id: &MenuId, quit_id: &MenuId) -> TrayIcon {
    let menu = Menu::new();
    menu.append(&MenuItem::with_id(
        open_id.clone(),
        "Open dashboard",
        true,
        None,
    ))
    .expect("appending a menu item should never fail");
    menu.append(&MenuItem::with_id(
        regenerate_id.clone(),
        "New code",
        true,
        None,
    ))
    .expect("appending a menu item should never fail");
    menu.append(&PredefinedMenuItem::separator())
        .expect("appending a menu item should never fail");
    menu.append(&MenuItem::with_id(quit_id.clone(), "Quit", true, None))
        .expect("appending a menu item should never fail");

    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(build_icon())
        .with_tooltip("PhoneInputConnect")
        .build()
        .expect("failed to create the tray icon")
}

struct App {
    callbacks: TrayCallbacks,
    open_id: MenuId,
    regenerate_id: MenuId,
    quit_id: MenuId,
    // Held for as long as the icon should stay visible; dropping it (e.g.
    // by never assigning it here) removes the icon.
    tray_icon: Option<TrayIcon>,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        // `StartCause::Init` (the first `resumed`) is the platform-blessed
        // place to create the icon; later `resumed` calls (e.g. after a
        // macOS suspend) leave the existing one alone.
        if self.tray_icon.is_none() {
            self.tray_icon = Some(build_tray_icon(
                &self.open_id,
                &self.regenerate_id,
                &self.quit_id,
            ));
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
        // No windows are ever created -- the dashboard is a browser tab,
        // not a native window -- so there is nothing to route here.
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::TrayClicked => (self.callbacks.open_dashboard)(),
            UserEvent::MenuClicked(id) => {
                if id == self.open_id {
                    (self.callbacks.open_dashboard)();
                } else if id == self.regenerate_id {
                    (self.callbacks.regenerate)();
                } else if id == self.quit_id {
                    (self.callbacks.quit)();
                }
            }
            UserEvent::Quit => event_loop.exit(),
        }
    }
}

/// Starts the pairing server and runs the tray icon's event loop on the
/// calling thread until "Quit" is chosen. Blocks until then.
pub fn run(make_callback: fn(Arc<Injector>) -> Arc<dyn Fn(String) + Send + Sync>) {
    let runtime = tokio::runtime::Runtime::new().expect("failed to start the async runtime");

    if let Some(url) = runtime.block_on(crate::instance::find_running_instance()) {
        log::info!("PhoneInputConnect is already running; opening its dashboard: {url}");
        let _ = webbrowser::open(&url);
        return;
    }

    let injector = Arc::new(Injector::new().expect("failed to initialize keyboard input injector"));
    let server = Arc::new(
        runtime
            .block_on(PairingServer::start(make_callback(injector)))
            .expect("failed to start pairing server"),
    );

    log::info!("Dashboard: {}", server.dashboard_url());
    crate::instance::record_running_instance(&server.dashboard_url());
    if webbrowser::open(&server.dashboard_url()).is_err() {
        log::warn!("Could not open a browser automatically; open the dashboard URL by hand.");
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to create the tray icon's event loop");
    event_loop.set_control_flow(ControlFlow::Wait);

    // tray-icon delivers menu/icon events via global callback handlers
    // (fired from OS-owned threads), not through winit -- forward them
    // onto the event loop as user events so they're handled alongside
    // everything else on this thread.
    let menu_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = menu_proxy.send_event(UserEvent::MenuClicked(event.id().clone()));
    }));
    let tray_proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = tray_proxy.send_event(UserEvent::TrayClicked);
        }
    }));

    let quit_proxy = event_loop.create_proxy();
    let callbacks = TrayCallbacks {
        open_dashboard: Box::new({
            let server = server.clone();
            move || {
                let _ = webbrowser::open(&server.dashboard_url());
            }
        }),
        regenerate: Box::new({
            let server = server.clone();
            move || server.regenerate_token()
        }),
        quit: Box::new(move || {
            let _ = quit_proxy.send_event(UserEvent::Quit);
        }),
    };

    let mut app = App {
        callbacks,
        open_id: MenuId::new("open-dashboard"),
        regenerate_id: MenuId::new("regenerate"),
        quit_id: MenuId::new("quit"),
        tray_icon: None,
    };

    event_loop
        .run_app(&mut app)
        .expect("tray icon event loop exited with an error");

    // `app`'s TrayIcon is dropped here (removing the icon) as `app` goes
    // out of scope, then the server is torn down before returning.
    runtime.block_on(server.stop());
}

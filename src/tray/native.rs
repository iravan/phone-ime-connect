//! The macOS native window, driven by a `winit` event loop that owns the
//! process's actual main thread -- required by macOS's native GUI plumbing
//! (a Cocoa run loop specifically has to pump on the main thread; see the
//! module-level note in `tray/mod.rs`). Linux and Windows have their own
//! native windows instead (`window/`).
//!
//! A menu-bar tray icon (`tray-icon`/`muda`) sits alongside the window with
//! a "Show", a checkable "Launch at login", and a "Quit" item; closing the
//! window hides it to the tray and leaves the pairing server running, so the
//! tray is the way back. The dashboard is drawn with native
//! AppKit widgets (`appkit_dashboard.rs`). It takes no network connection --
//! the same in-process snapshot stream the Linux window uses
//! (`PairingServer::subscribe_dashboard`) is pushed straight into the
//! widgets, and the "New code" button calls back in-process. No external
//! browser is opened.
//!
//! Because the event loop owns this thread, the pairing server instead
//! runs on a background Tokio runtime: it's started with one `block_on`
//! call before the event loop starts (so the dashboard state is known up
//! front), and its worker threads then keep driving connections
//! independently of this thread until `run_app` returns.

use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
#[cfg(target_os = "macos")]
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::{Window, WindowId};

use super::appkit_dashboard::Content;
use crate::injector::{InputEvent, Injector};
use crate::server::PairingServer;

/// Events routed onto the winit event loop from elsewhere: a fresh dashboard
/// snapshot from the server's background task, and a received message to
/// inject.
enum UserEvent {
    Snapshot(String),
    /// A message from the phone, to be typed into the focused window. Run on
    /// the event loop's thread (the process main thread) because macOS's
    /// text-input APIs, which `enigo` consults to resolve the paste
    /// keystroke, assert they run there -- calling the injector from the
    /// server's blocking thread instead aborts the process.
    Inject(InputEvent),
    /// A menu-bar tray item was chosen (carries the item's id).
    Menu(MenuId),
}

struct App {
    server: Arc<PairingServer>,
    injector: Arc<Injector>,
    // The dashboard content may hold a handle into `window`, so it must drop
    // first: struct fields drop in declaration order. Both are `None` until
    // the event loop's first `resumed`.
    content: Option<Content>,
    window: Option<Window>,
    // Menu-bar tray, built on the first `resumed` (it needs a running
    // NSApplication). `tray` is only held to keep the icon alive; the
    // `autostart` check item is kept to flip its checkmark; `show_id`/
    // `quit_id` identify their menu items when a `Menu` event arrives.
    tray: Option<TrayIcon>,
    autostart: Option<CheckMenuItem>,
    show_id: Option<MenuId>,
    quit_id: Option<MenuId>,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // The first `resumed` (`StartCause::Init`) is the platform-blessed
        // place to create native GUI objects; later `resumed` calls (e.g.
        // after a macOS suspend) leave the existing ones alone.
        if self.window.is_none() {
            let window = event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("PhoneInputConnect")
                        .with_inner_size(LogicalSize::new(420.0, 640.0)),
                )
                .expect("failed to create the native window");

            let content = Content::create(&window, self.server.clone());
            // Paint the current state right away; later changes arrive as
            // `UserEvent::Snapshot`.
            content.apply_snapshot(&self.server.dashboard_snapshot());
            content.set_accessibility_trusted(accessibility_trusted());

            self.content = Some(content);
            self.window = Some(window);
            // Shrink to just the idle contents now that they're laid out.
            self.fit_window();

            // The menu-bar tray needs a live NSApplication, so it's built
            // here (first `resumed`) rather than before the event loop, and
            // only once. Menu clicks arrive as `UserEvent::Menu` via the
            // handler installed in `run`.
            self.build_tray();
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            // With a menu-bar tray to restore from, closing just hides the
            // window and leaves the pairing server running -- reopen via the
            // tray's "Show" item. "Quit" (also in the tray) is what actually
            // exits.
            WindowEvent::CloseRequested => {
                if let Some(window) = &self.window {
                    window.set_visible(false);
                }
            }
            // Returning to the window (e.g. after granting the permission in
            // System Settings) re-checks Accessibility so the notice clears.
            WindowEvent::Focused(true) => {
                if let Some(content) = &self.content {
                    content.set_accessibility_trusted(accessibility_trusted());
                }
                self.fit_window();
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Snapshot(json) => {
                if let Some(content) = &self.content {
                    content.apply_snapshot(&json);
                    content.set_accessibility_trusted(accessibility_trusted());
                }
                // Re-fit: the log section appearing/disappearing changes the
                // content height (see `Content::fitting_size`).
                self.fit_window();
            }
            UserEvent::Inject(event) => self.injector.dispatch(event),
            UserEvent::Menu(id) => {
                if self.show_id.as_ref() == Some(&id) {
                    self.show_window();
                } else if self.quit_id.as_ref() == Some(&id) {
                    event_loop.exit();
                } else if let Some(item) = &self.autostart {
                    if id == *item.id() {
                        // `is_checked` already reflects the click; persist
                        // that, then correct the checkmark to the state that
                        // actually took effect (in case the write failed).
                        let took = crate::autostart::set_enabled(item.is_checked());
                        item.set_checked(took);
                    }
                }
            }
        }
    }
}

impl App {
    /// Builds the menu-bar tray icon and its menu. Called once on the first
    /// `resumed`. Failures here are non-fatal: the window still works, just
    /// without a tray, so they're logged rather than panicked on.
    fn build_tray(&mut self) {
        let s = crate::i18n::strings();

        let icon = match tray_icon_image() {
            Some(icon) => icon,
            None => return,
        };

        let show = MenuItem::new(s.menu_show, true, None);
        let autostart = CheckMenuItem::new(
            s.menu_launch_at_login,
            true,
            crate::autostart::is_enabled(),
            None,
        );
        let quit = MenuItem::new(s.menu_quit, true, None);

        let menu = Menu::new();
        if let Err(err) = menu.append_items(&[
            &show,
            &PredefinedMenuItem::separator(),
            &autostart,
            &PredefinedMenuItem::separator(),
            &quit,
        ]) {
            log::warn!("failed to build the tray menu: {err}");
            return;
        }

        match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("PhoneInputConnect")
            .with_icon(icon)
            .build()
        {
            Ok(tray) => {
                self.show_id = Some(show.id().clone());
                self.quit_id = Some(quit.id().clone());
                self.autostart = Some(autostart);
                self.tray = Some(tray);
            }
            Err(err) => log::warn!("failed to create the tray icon: {err}"),
        }
    }

    /// Resizes the window down to just fit its current contents, so the idle
    /// window (status + QR + hint + button) is a small rectangle and only
    /// grows when the message log or Accessibility notice appears. Width is
    /// held steady; only the height tracks the content.
    fn fit_window(&self) {
        if let (Some(window), Some(content)) = (&self.window, &self.content) {
            let (_, h) = content.fitting_size();
            // Floor guards against a degenerate size before first layout.
            let _ = window.request_inner_size(LogicalSize::new(420.0, h.max(240.0)));
        }
    }

    /// Un-hides and re-focuses the window (from the tray's "Show" item).
    fn show_window(&self) {
        if let Some(window) = &self.window {
            window.set_visible(true);
            window.focus_window();
        }
    }
}

/// Decodes the bundled PNG app icon into a `tray-icon` RGBA icon. `None` on
/// any decode failure (the tray is then skipped).
fn tray_icon_image() -> Option<tray_icon::Icon> {
    let img = image::load_from_memory(include_bytes!("../../assets/icon-256.png"))
        .ok()?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()
}

/// Whether this process currently has macOS Accessibility trust, without
/// prompting -- used to drive the in-window "grant Accessibility" notice.
#[cfg(target_os = "macos")]
fn accessibility_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    // SAFETY: a documented, argument-free CoreFoundation predicate.
    unsafe { AXIsProcessTrusted() }
}

/// Requests macOS Accessibility trust, which the synthetic paste keystroke
/// needs -- without it, delivery silently types nothing. When not already
/// trusted this pops the system "allow ... to control this computer" dialog
/// and registers this binary in the Accessibility list. The trust is keyed
/// to the exact binary, so a rebuild drops a previously-granted permission
/// (and this prompts again).
#[cfg(target_os = "macos")]
fn request_accessibility_trust() {
    use std::ffi::c_void;
    use std::ptr;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        static kAXTrustedCheckOptionPrompt: *const c_void; // CFStringRef
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

    // SAFETY: a standard CoreFoundation dictionary of one well-known
    // CFString key -> CFBoolean value, passed to the documented
    // AXIsProcessTrustedWithOptions and released right after.
    let trusted = unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        let trusted = AXIsProcessTrustedWithOptions(options);
        CFRelease(options);
        trusted
    };

    if trusted {
        log::info!("macOS Accessibility: trusted -- pasting into other apps is enabled.");
    } else {
        log::warn!(
            "macOS Accessibility: NOT trusted -- approve the permission prompt (or enable \
             this binary under System Settings > Privacy & Security > Accessibility), then \
             relaunch. Messages still arrive meanwhile, but typing into other apps does \
             nothing. (Rebuilding the binary invalidates a previous grant.)"
        );
    }
}

/// Starts the pairing server and runs the native window's event loop on the
/// calling thread until the window is closed. Blocks until then.
pub fn run() {
    let runtime = tokio::runtime::Runtime::new().expect("failed to start the async runtime");

    if runtime.block_on(crate::instance::find_running_instance()) {
        log::info!("PhoneInputConnect is already running; its window should already be on screen.");
        return;
    }

    let mut event_loop_builder = EventLoop::<UserEvent>::with_user_event();
    // Now that a real window is the primary UI, run as a regular app: a
    // bare binary otherwise defaults to a Dock-less activation policy where
    // the window never becomes key normally, which trips a winit
    // resign-key crash on macOS.
    #[cfg(target_os = "macos")]
    event_loop_builder.with_activation_policy(ActivationPolicy::Regular);
    let event_loop = event_loop_builder
        .build()
        .expect("failed to create the event loop");
    event_loop.set_control_flow(ControlFlow::Wait);

    #[cfg(target_os = "macos")]
    request_accessibility_trust();

    // The event loop is built before the server so its proxy exists first:
    // the server invokes `on_message` from a blocking thread, but the
    // injector must run on this (main) thread, so the message is forwarded
    // there as a `UserEvent::Inject`.
    // Route tray menu clicks onto the event loop as `UserEvent::Menu`, so
    // they're handled on this (main) thread alongside everything else.
    let menu_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = menu_proxy.send_event(UserEvent::Menu(event.id));
    }));

    let injector = Arc::new(Injector::new().expect("failed to initialize keyboard input injector"));
    let inject_proxy = event_loop.create_proxy();
    let on_message: Arc<dyn Fn(InputEvent) + Send + Sync> = Arc::new(move |event: InputEvent| {
        let _ = inject_proxy.send_event(UserEvent::Inject(event));
    });
    let server = Arc::new(
        runtime
            .block_on(PairingServer::start(on_message))
            .expect("failed to start pairing server"),
    );

    log::info!("Pairing server listening at {}", server.lan_socket_addr());
    crate::instance::record_running_instance(&server.lan_socket_addr().to_string());

    // Forward each live dashboard snapshot from the server's background task
    // onto the event loop, where it can touch the (main-thread-only)
    // webview. Lagging just means we skipped intermediate states; the next
    // snapshot is still current, so keep going.
    let snapshot_proxy = event_loop.create_proxy();
    let mut snapshots = server.subscribe_dashboard();
    runtime.spawn(async move {
        loop {
            match snapshots.recv().await {
                Ok(json) => {
                    if snapshot_proxy.send_event(UserEvent::Snapshot(json)).is_err() {
                        break; // event loop is gone
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });

    let mut app = App {
        server: server.clone(),
        injector,
        content: None,
        window: None,
        tray: None,
        autostart: None,
        show_id: None,
        quit_id: None,
    };

    event_loop
        .run_app(&mut app)
        .expect("event loop exited with an error");

    // `app`'s window is dropped here as it goes out of scope, then the server
    // is torn down before returning.
    runtime.block_on(server.stop());
}

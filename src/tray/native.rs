//! The macOS native window + tray/menu-bar icon, driven by a `winit` event
//! loop that owns the process's actual main thread -- required by macOS's
//! native GUI plumbing (a Cocoa run loop specifically has to pump on the
//! main thread; see the module-level note in `tray/mod.rs`). Linux and
//! Windows have their own native windows instead (`window/`).
//!
//! The dashboard is drawn with native AppKit widgets (`appkit_dashboard.rs`).
//! It takes no network connection -- the same in-process snapshot stream the
//! Linux window uses (`PairingServer::subscribe_dashboard`) is pushed
//! straight into the widgets, and the "New code" button calls back
//! in-process. No external browser is opened.
//!
//! Because the event loop owns this thread, the pairing server instead
//! runs on a background Tokio runtime: it's started with one `block_on`
//! call before the event loop starts (so the dashboard state is known up
//! front), and its worker threads then keep driving connections
//! independently of this thread until `run_app` returns.

use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
#[cfg(target_os = "macos")]
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::{Window, WindowId};

use super::appkit_dashboard::Content;
use crate::injector::Injector;
use crate::server::PairingServer;

/// Events routed onto the winit event loop from elsewhere: the global
/// muda/tray-icon callback handlers (which fire on an OS-owned thread), a
/// fresh dashboard snapshot from the server's background task, and a
/// received message to inject.
enum UserEvent {
    MenuClicked(MenuId),
    TrayClicked,
    Snapshot(String),
    /// A message from the phone, to be typed into the focused window. Run on
    /// the event loop's thread (the process main thread) because macOS's
    /// text-input APIs, which `enigo` consults to resolve the paste
    /// keystroke, assert they run there -- calling the injector from the
    /// server's blocking thread instead aborts the process.
    Inject(String),
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
    let s = crate::i18n::strings();
    let menu = Menu::new();
    menu.append(&MenuItem::with_id(
        open_id.clone(),
        s.tray_show_window,
        true,
        None,
    ))
    .expect("appending a menu item should never fail");
    menu.append(&MenuItem::with_id(
        regenerate_id.clone(),
        s.button_new_code,
        true,
        None,
    ))
    .expect("appending a menu item should never fail");
    menu.append(&PredefinedMenuItem::separator())
        .expect("appending a menu item should never fail");
    menu.append(&MenuItem::with_id(quit_id.clone(), s.tray_quit, true, None))
        .expect("appending a menu item should never fail");

    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(build_icon())
        .with_tooltip("PhoneInputConnect")
        .build()
        .expect("failed to create the tray icon")
}

struct App {
    server: Arc<PairingServer>,
    injector: Arc<Injector>,
    open_id: MenuId,
    regenerate_id: MenuId,
    quit_id: MenuId,
    // The dashboard content may hold a handle into `window` (the webview
    // does), so it must drop first: struct fields drop in declaration
    // order. Both are `None` until the event loop's first `resumed`.
    content: Option<Content>,
    window: Option<Window>,
    // Held for as long as the icon should stay visible; dropping it removes
    // the icon.
    tray_icon: Option<TrayIcon>,
}

impl App {
    /// Brings the window back after it's been closed (hidden) or is behind
    /// other windows.
    fn show_window(&self) {
        if let Some(window) = &self.window {
            window.set_visible(true);
            window.focus_window();
        }
        // Re-check Accessibility when the window comes forward, so granting it
        // (then reopening from the tray) clears the in-window notice.
        if let Some(content) = &self.content {
            content.set_accessibility_trusted(accessibility_trusted());
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // The first `resumed` (`StartCause::Init`) is the platform-blessed
        // place to create native GUI objects; later `resumed` calls (e.g.
        // after a macOS suspend) leave the existing ones alone.
        if self.tray_icon.is_none() {
            self.tray_icon = Some(build_tray_icon(
                &self.open_id,
                &self.regenerate_id,
                &self.quit_id,
            ));
        }
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
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Closing the window hides it rather than quitting -- the tray icon
        // stays, "Show window" brings it back, and "Quit" is the real exit.
        if let WindowEvent::CloseRequested = event {
            if let Some(window) = &self.window {
                window.set_visible(false);
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::TrayClicked => self.show_window(),
            UserEvent::MenuClicked(id) => {
                if id == self.open_id {
                    self.show_window();
                } else if id == self.regenerate_id {
                    self.server.regenerate_token();
                } else if id == self.quit_id {
                    event_loop.exit();
                }
            }
            UserEvent::Snapshot(json) => {
                if let Some(content) = &self.content {
                    content.apply_snapshot(&json);
                    content.set_accessibility_trusted(accessibility_trusted());
                }
            }
            UserEvent::Inject(text) => self.injector.type_text(&text),
        }
    }
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

/// Starts the pairing server and runs the native window + tray icon's event
/// loop on the calling thread until "Quit" is chosen. Blocks until then.
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
    let injector = Arc::new(Injector::new().expect("failed to initialize keyboard input injector"));
    let inject_proxy = event_loop.create_proxy();
    let on_message: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |text: String| {
        let _ = inject_proxy.send_event(UserEvent::Inject(text));
    });
    let server = Arc::new(
        runtime
            .block_on(PairingServer::start(on_message))
            .expect("failed to start pairing server"),
    );

    log::info!("Pairing server listening at {}", server.lan_socket_addr());
    crate::instance::record_running_instance(&server.lan_socket_addr().to_string());

    // tray-icon delivers menu/icon events via global callback handlers
    // (fired from OS-owned threads), not through winit -- forward them onto
    // the event loop as user events so they're handled alongside everything
    // else on this thread.
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
        open_id: MenuId::new("show-window"),
        regenerate_id: MenuId::new("regenerate"),
        quit_id: MenuId::new("quit"),
        content: None,
        window: None,
        tray_icon: None,
    };

    event_loop
        .run_app(&mut app)
        .expect("event loop exited with an error");

    // `app`'s webview/window/tray are dropped here as it goes out of scope,
    // then the server is torn down before returning.
    runtime.block_on(server.stop());
}

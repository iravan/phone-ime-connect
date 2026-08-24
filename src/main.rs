mod injector;
mod instance;
mod lan;
mod qr;
mod server;
mod tls;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod window;

#[cfg(target_os = "macos")]
mod tray;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::sync::Arc;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use server::PairingServer;

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn on_message_callback(injector: Arc<injector::Injector>) -> Arc<dyn Fn(String) + Send + Sync> {
    Arc::new(move |text: String| injector.type_text(&text))
}

/// Shared by Linux and Windows, whose only real difference is which
/// native-window toolkit actually renders the UI (`window_run`).
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run_with_native_window(
    window_run: impl FnOnce(Arc<tokio::runtime::Runtime>, Arc<PairingServer>),
) {
    env_logger::init();

    let runtime =
        Arc::new(tokio::runtime::Runtime::new().expect("failed to start the async runtime"));

    if runtime.block_on(instance::find_running_instance()) {
        log::info!("PhoneInputConnect is already running; its window should already be on screen.");
        return;
    }

    let injector =
        Arc::new(injector::Injector::new().expect("failed to initialize keyboard input injector"));
    let server = Arc::new(
        runtime
            .block_on(PairingServer::start(on_message_callback(injector)))
            .expect("failed to start pairing server"),
    );

    log::info!("Pairing server listening at {}", server.lan_socket_addr());
    instance::record_running_instance(&server.lan_socket_addr().to_string());

    // Blocks until the window is closed.
    window_run(runtime.clone(), server.clone());

    runtime.block_on(server.stop());
}

#[cfg(target_os = "linux")]
fn main() {
    run_with_native_window(window::linux::run);
}

#[cfg(target_os = "windows")]
fn main() {
    run_with_native_window(window::windows::run);
}

#[cfg(target_os = "macos")]
fn main() {
    env_logger::init();
    tray::native::run();
}

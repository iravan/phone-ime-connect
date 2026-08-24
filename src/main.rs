mod injector;
mod instance;
mod lan;
mod qr;
mod server;
mod tls;
#[cfg(target_os = "linux")]
mod window;

#[cfg(not(target_os = "linux"))]
mod tray;

use std::sync::Arc;

#[cfg(target_os = "linux")]
use server::PairingServer;

fn on_message_callback(injector: Arc<injector::Injector>) -> Arc<dyn Fn(String) + Send + Sync> {
    Arc::new(move |text: String| injector.type_text(&text))
}

#[cfg(target_os = "linux")]
fn main() {
    env_logger::init();

    let runtime =
        Arc::new(tokio::runtime::Runtime::new().expect("failed to start the async runtime"));

    if let Some(url) = runtime.block_on(instance::find_running_instance()) {
        log::info!(
            "PhoneInputConnect is already running; its dashboard is at {url} if you need it, \
             but its window should already be on screen."
        );
        return;
    }

    let injector =
        Arc::new(injector::Injector::new().expect("failed to initialize keyboard input injector"));
    let server = Arc::new(
        runtime
            .block_on(PairingServer::start(on_message_callback(injector)))
            .expect("failed to start pairing server"),
    );

    log::info!("Dashboard: {}", server.dashboard_url());
    instance::record_running_instance(&server.dashboard_url());

    // Blocks until the window is closed.
    window::run(runtime.clone(), server.clone());

    runtime.block_on(server.stop());
}

#[cfg(not(target_os = "linux"))]
fn main() {
    env_logger::init();
    tray::native::run(on_message_callback);
}

mod injector;
mod lan;
mod qr;
mod server;
mod tls;
mod tray;

use std::sync::Arc;

use server::PairingServer;

fn on_message_callback(injector: Arc<injector::Injector>) -> Arc<dyn Fn(String) + Send + Sync> {
    Arc::new(move |text: String| injector.type_text(&text))
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() {
    env_logger::init();

    let injector =
        Arc::new(injector::Injector::new().expect("failed to initialize keyboard input injector"));
    let server = Arc::new(
        PairingServer::start(on_message_callback(injector))
            .await
            .expect("failed to start pairing server"),
    );

    log::info!("Dashboard: {}", server.dashboard_url());
    if webbrowser::open(&server.dashboard_url()).is_err() {
        log::warn!("Could not open a browser automatically; open the dashboard URL by hand.");
    }

    let quit_notify = Arc::new(tokio::sync::Notify::new());
    let callbacks = {
        let server = server.clone();
        let quit_notify_for_quit = quit_notify.clone();
        tray::TrayCallbacks {
            open_dashboard: Box::new({
                let server = server.clone();
                move || {
                    let _ = webbrowser::open(&server.dashboard_url());
                }
            }),
            regenerate: Box::new(move || server.regenerate_token()),
            quit: Box::new(move || quit_notify_for_quit.notify_waiters()),
        }
    };
    let tray_handle = tray::linux::spawn(callbacks)
        .await
        .expect("failed to start tray icon");

    tokio::select! {
        _ = quit_notify.notified() => {}
        _ = tokio::signal::ctrl_c() => {}
    }

    tray_handle.shutdown().await;
    server.stop().await;
}

#[cfg(not(target_os = "linux"))]
fn main() {
    env_logger::init();
    tray::native::run(on_message_callback);
}

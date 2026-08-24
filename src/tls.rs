//! Generates and caches a self-signed TLS certificate for the local
//! pairing server, using `rcgen` -- pure Rust, no OpenSSL install needed
//! on any of the three target platforms (Windows in particular has no
//! `openssl` binary on PATH by default).
//!
//! This certificate's only job is opportunistic encryption of the LAN hop
//! between the phone and this machine -- there is no certificate
//! authority behind it, so the phone's browser will show a one-time
//! "connection is not private" warning that the user has to click
//! through once per phone. A long validity period avoids regenerating it
//! (and re-triggering that warning with a new fingerprint) on every app
//! restart; it's still per-machine, cached under this user's private app
//! data directory.

use std::path::PathBuf;

use axum_server::tls_rustls::RustlsConfig;
use directories::ProjectDirs;
use rcgen::{generate_simple_self_signed, CertifiedKey};

fn state_dir() -> std::io::Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "phone-input-connect").ok_or_else(|| {
        std::io::Error::other("could not determine a per-user app data directory")
    })?;
    let dir = dirs.data_local_dir().to_path_buf();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn generate(cert_path: &PathBuf, key_path: &PathBuf) -> std::io::Result<()> {
    let CertifiedKey { cert, key_pair } =
        generate_simple_self_signed(vec!["phone-input-connect.local".to_string()])
            .map_err(std::io::Error::other)?;
    std::fs::write(cert_path, cert.pem())?;
    std::fs::write(key_path, key_pair.serialize_pem())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))?;
        std::fs::set_permissions(cert_path, std::fs::Permissions::from_mode(0o600))?;
    }
    // On Windows, the per-user AppData folder this lives under is already
    // restricted to that user by the filesystem ACLs -- no extra
    // permission tightening needed.
    Ok(())
}

/// Loads the cached self-signed certificate, generating one on first use,
/// and returns it as a ready-to-use `RustlsConfig`.
pub async fn load_or_create_config() -> std::io::Result<RustlsConfig> {
    let dir = state_dir()?;
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    if !(cert_path.exists() && key_path.exists()) {
        generate(&cert_path, &key_path)?;
    }
    RustlsConfig::from_pem_file(&cert_path, &key_path).await
}

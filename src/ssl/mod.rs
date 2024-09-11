// src/ssl/mod.rs
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::TlsAcceptor;
use std::sync::Arc;

pub async fn load_ssl_config() -> Result<ServerConfig, Box<dyn std::error::Error>> {
    let cert_path = "/etc/pki/tls/certs/wss.gabler.app.crt";
    let key_path = "/etc/pki/tls/certs/wss.gabler.app.key";
    let mut cert_file = File::open(cert_path).await?;
    let mut key_file = File::open(key_path).await?;
    let mut cert_data = Vec::new();
    let mut key_data = Vec::new();
    cert_file.read_to_end(&mut cert_data).await?;
    key_file.read_to_end(&mut key_data).await?;

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_data.as_slice())
        .collect::<Result<_, _>>()?;
    let key = rustls_pemfile::pkcs8_private_keys(&mut key_data.as_slice())
        .next()
        .ok_or("No private key found")?
        .map(PrivateKeyDer::Pkcs8)?;

    // Create a configuration supporting TLS 1.3 and 1.2
    // TLS 1.3 is preferred and used by default when available
    let crypto_provider = rustls::crypto::aws_lc_rs::default_provider();
    let config = ServerConfig::builder_with_provider(Arc::new(crypto_provider))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    Ok(config)
}

pub fn create_tls_acceptor(config: ServerConfig) -> TlsAcceptor {
    TlsAcceptor::from(std::sync::Arc::new(config))
}

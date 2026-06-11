//! TLS configuration for ice0 connections.
//!
//! Provides utilities to create TLS acceptors (server) and connectors (client)
//! for encrypted ice0 communication.

use std::io::{BufReader, Cursor};
use std::sync::Arc;
use std::path::Path;
use anyhow::{anyhow, Result};
use log::info;

use tokio_rustls::rustls::{self, pki_types::{CertificateDer, PrivateKeyDer}};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// Load certificates from a PEM file
pub fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    info!("Loading certificates from: {}", path.display());
    let data = std::fs::read_to_string(path)?;
    let mut reader = BufReader::new(Cursor::new(data));
    
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()?;
    
    info!("Loaded {} certificate(s)", certs.len());
    Ok(certs)
}

/// Load private key from a PEM file
pub fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    info!("Loading private key from: {}", path.display());
    let data = std::fs::read_to_string(path)?;
    let mut reader = BufReader::new(Cursor::new(data));
    
    loop {
        match rustls_pemfile::read_one(&mut reader)? {
            Some(rustls_pemfile::Item::Pkcs1Key(key)) => return Ok(key.into()),
            Some(rustls_pemfile::Item::Pkcs8Key(key)) => return Ok(key.into()),
            Some(rustls_pemfile::Item::Sec1Key(key)) => return Ok(key.into()),
            None => break,
            _ => continue,
        }
    }
    
    Err(anyhow!("No private key found in {}", path.display()))
}

/// Create a TLS acceptor for the server
pub fn create_tls_acceptor(cert_path: &Path, key_path: &Path) -> Result<TlsAcceptor> {
    let cert = load_certs(cert_path)?;
    let key = load_private_key(key_path)?;
    
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert, key)?;
    
    info!("TLS acceptor created successfully");
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// Create a TLS connector for the client
pub fn create_tls_connector(server_cert_path: &Path) -> Result<TlsConnector> {
    let mut root_store = rustls::RootCertStore::empty();
    
    let certs = load_certs(server_cert_path)?;
    for cert in certs {
        root_store.add(cert)?;
    }
    
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    
    info!("TLS connector created successfully");
    Ok(TlsConnector::from(Arc::new(config)))
}

/// Get default certificate paths
pub fn get_default_cert_paths() -> (String, String) {
    // Try multiple locations for certificates
    let cert_paths = vec![
        "certs/server.crt",
        "../certs/server.crt",
        "/etc/rtcdesk/certs/server.crt",
    ];
    
    let key_paths = vec![
        "certs/server.key",
        "../certs/server.key",
        "/etc/rtcdesk/certs/server.key",
    ];
    
    // Find first existing cert
    let cert_path = cert_paths.iter()
        .find(|p| Path::new(p).exists())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "certs/server.crt".to_string());
    
    // Find first existing key
    let key_path = key_paths.iter()
        .find(|p| Path::new(p).exists())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "certs/server.key".to_string());
    
    (cert_path, key_path)
}

/// Get default server certificate path for clients
pub fn get_default_server_cert_path() -> String {
    let paths = vec![
        "certs/server.crt",
        "../certs/server.crt",
        "/etc/rtcdesk/certs/server.crt",
    ];
    
    paths.iter()
        .find(|p| Path::new(p).exists())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "certs/server.crt".to_string())
}

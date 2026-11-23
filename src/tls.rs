use anyhow::{anyhow, bail, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as TlsError, RootCertStore, SignatureScheme};
use std::fs;
use std::io::Cursor;
use std::sync::Arc;

use crate::cert;

pub fn parse_server_name(name: &str) -> Result<ServerName<'static>> {
    ServerName::try_from(name.to_owned())
        .map_err(|err| anyhow!("invalid server name {name:?}: {err}"))
}

pub fn build_client_config(
    ca: &str,
    cert_hash: &str,
    insecure: bool,
    grpc: bool,
) -> Result<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let pin = if cert_hash.is_empty() {
        None
    } else {
        let pin = hex::decode(cert_hash).context("invalid cert hash")?;
        if pin.len() > 32 {
            bail!("invalid cert hash: hash is longer than 32 bytes");
        }
        Some(pin)
    };

    let roots = load_roots(ca)?;
    let inner = if pin.is_none() && !insecure {
        if roots.is_empty() {
            bail!("no trusted certificates available");
        }
        Some(
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                .build()
                .context("failed to build certificate verifier")?,
        )
    } else {
        None
    };

    let verifier = Arc::new(PinVerifier {
        inner,
        pin,
        provider: provider.clone(),
    });

    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("failed to configure TLS 1.3")?;
    let mut config = builder
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    if grpc {
        config.alpn_protocols = vec![b"h2".to_vec()];
    }

    Ok(Arc::new(config))
}

pub fn build_server_config(
    cert_pem: &[u8],
    key_pem: &[u8],
    grpc: bool,
) -> Result<Arc<rustls::ServerConfig>> {
    let certs = cert::parse_pem_certs(cert_pem)?;
    let key = rustls_pemfile::private_key(&mut Cursor::new(key_pem))
        .context("cannot parse private key")?
        .ok_or_else(|| anyhow!("no private key found"))?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("failed to configure TLS 1.3")?;
    let mut config = builder
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("cannot load x509 key pair")?;

    if grpc {
        config.alpn_protocols = vec![b"h2".to_vec()];
    }

    Ok(Arc::new(config))
}

fn load_roots(ca: &str) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();

    if !ca.is_empty() {
        let data = fs::read(ca).with_context(|| format!("cannot read ca file: {ca}"))?;
        let certs = cert::parse_pem_certs(&data)
            .context("no valid certificate was found in the ca file")?;
        for cert in certs {
            roots
                .add(cert)
                .context("failed to add CA certificate to the root store")?;
        }
        return Ok(roots);
    }

    let native = rustls_native_certs::load_native_certs();
    for error in &native.errors {
        log::debug!("failed to load native certificate: {error}");
    }
    for cert in native.certs {
        if let Err(error) = roots.add(cert) {
            log::debug!("failed to add native certificate: {error}");
        }
    }

    if roots.is_empty() {
        log::debug!("no native certificates found, using bundled webpki roots");
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }

    Ok(roots)
}

#[derive(Debug)]
struct PinVerifier {
    inner: Option<Arc<WebPkiServerVerifier>>,
    pin: Option<Vec<u8>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, TlsError> {
        if let Some(pin) = &self.pin {
            let hash = cert::tbs_sha256(end_entity.as_ref()).map_err(|err| {
                TlsError::General(format!("cannot parse server certificate: {err}"))
            })?;
            if hash[..pin.len()] == pin[..] {
                return Ok(ServerCertVerified::assertion());
            }
            return Err(TlsError::General(format!(
                "cert hash mismatch, received cert hash is [{}]",
                hex::encode(hash)
            )));
        }

        if let Some(inner) = &self.inner {
            return inner.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            );
        }

        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

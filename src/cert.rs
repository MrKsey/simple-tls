use ::time::{Duration, OffsetDateTime};
use anyhow::{anyhow, bail, Context, Result};
use rand::RngCore;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SerialNumber, PKCS_ECDSA_P256_SHA256,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use x509_parser::prelude::*;

pub struct GeneratedCert {
    pub dns_name: String,
    pub key_pem: String,
    pub cert_pem: String,
    pub cert_der: Vec<u8>,
    pub tbs_sha256: [u8; 32],
}

struct TemplateInfo {
    dns_names: Vec<String>,
    ips: Vec<IpAddr>,
    common_name: Option<String>,
    not_before: Option<i64>,
}

pub fn generate_cert(server_name: &str, template_path: Option<&Path>) -> Result<GeneratedCert> {
    let template = match template_path {
        Some(path) if !path.as_os_str().is_empty() => Some(load_template(path)?),
        _ => None,
    };

    let dns_name = template
        .as_ref()
        .and_then(|template| template.dns_names.first().cloned())
        .unwrap_or_else(|| {
            if server_name.is_empty() {
                random_server_name()
            } else {
                server_name.to_owned()
            }
        });

    let mut dns_names = template
        .as_ref()
        .map(|template| template.dns_names.clone())
        .unwrap_or_default();
    if dns_names.is_empty() {
        dns_names.push(dns_name.clone());
    }

    let mut params = CertificateParams::new(dns_names)?;
    if let Some(template) = &template {
        for ip in &template.ips {
            params
                .subject_alt_names
                .push(rcgen::SanType::IpAddress(*ip));
        }
    }

    let common_name = template
        .as_ref()
        .and_then(|template| template.common_name.clone())
        .unwrap_or_else(|| dns_name.clone());
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, common_name);
    params.distinguished_name = distinguished_name;

    let now = OffsetDateTime::now_utc();
    params.not_before = template
        .as_ref()
        .and_then(|template| template.not_before)
        .and_then(|timestamp| OffsetDateTime::from_unix_timestamp(timestamp).ok())
        .unwrap_or(now);
    params.not_after = now + Duration::days(3650);
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![
        KeyUsagePurpose::KeyEncipherment,
        KeyUsagePurpose::DigitalSignature,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let mut serial = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut serial);
    params.serial_number = Some(SerialNumber::from_slice(&serial));

    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let cert = params.self_signed(&key_pair)?;
    let cert_der = cert.der().to_vec();
    let tbs_sha256 = tbs_sha256(&cert_der)?;

    Ok(GeneratedCert {
        dns_name,
        key_pem: key_pair.serialize_pem(),
        cert_pem: cert.pem(),
        cert_der,
        tbs_sha256,
    })
}

fn load_template(path: &Path) -> Result<TemplateInfo> {
    let data =
        fs::read(path).with_context(|| format!("cannot read template {}", path.display()))?;
    let certs = parse_pem_certs(&data)
        .with_context(|| format!("cannot parse template certificate {}", path.display()))?;
    let der = certs
        .first()
        .ok_or_else(|| anyhow!("no certificate found in template {}", path.display()))?;
    let (_, parsed) = X509Certificate::from_der(der.as_ref())
        .map_err(|err| anyhow!("cannot parse template certificate: {err}"))?;

    let mut dns_names = Vec::new();
    let mut ips = Vec::new();
    if let Ok(Some(san)) = parsed.subject_alternative_name() {
        for name in &san.value.general_names {
            match name {
                GeneralName::DNSName(dns_name) => dns_names.push((*dns_name).to_owned()),
                GeneralName::IPAddress(bytes) => {
                    if let Some(ip) = ip_from_octets(bytes) {
                        ips.push(ip);
                    }
                }
                _ => {}
            }
        }
    }

    let common_name = parsed
        .subject()
        .iter_common_name()
        .next()
        .and_then(|attribute| attribute.as_str().ok())
        .map(ToOwned::to_owned);

    Ok(TemplateInfo {
        dns_names,
        ips,
        common_name,
        not_before: Some(parsed.validity().not_before.timestamp()),
    })
}

fn ip_from_octets(bytes: &[u8]) -> Option<IpAddr> {
    match bytes.len() {
        4 => Some(IpAddr::V4(Ipv4Addr::new(
            bytes[0], bytes[1], bytes[2], bytes[3],
        ))),
        16 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(bytes);
            Some(IpAddr::V6(Ipv6Addr::from(octets)))
        }
        _ => None,
    }
}

pub fn parse_pem_certs(data: &[u8]) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let mut reader = Cursor::new(data);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("invalid PEM data")?;
    if certs.is_empty() {
        bail!("no valid certificate was found");
    }
    Ok(certs)
}

pub fn tbs_sha256(cert_der: &[u8]) -> Result<[u8; 32]> {
    let (_, parsed) = X509Certificate::from_der(cert_der)
        .map_err(|err| anyhow!("cannot parse certificate: {err}"))?;
    let mut hasher = Sha256::new();
    hasher.update(parsed.tbs_certificate.as_ref());
    Ok(hasher.finalize().into())
}

pub fn print_cert_hashes(path: &Path) -> Result<()> {
    let data =
        fs::read(path).with_context(|| format!("failed to read cert file {}", path.display()))?;
    let certs = parse_pem_certs(&data)?;
    for cert in certs {
        let (_, parsed) = X509Certificate::from_der(cert.as_ref())
            .map_err(|err| anyhow!("failed to parse cert file: {err}"))?;
        let hash = tbs_sha256(cert.as_ref())?;
        println!("[{}]: {}", parsed.subject(), hex::encode(hash));
    }
    Ok(())
}

fn random_server_name() -> String {
    let mut rng = rand::thread_rng();
    let first_len = (rng.next_u32() % 5 + 3) as usize;
    let second_len = (rng.next_u32() % 3 + 1) as usize;
    format!(
        "{}.{}",
        random_string(first_len, &mut rng),
        random_string(second_len, &mut rng)
    )
}

fn random_string(length: usize, rng: &mut impl RngCore) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    let mut value = String::with_capacity(length);
    for _ in 0..length {
        let index = (rng.next_u32() as usize) % ALPHABET.len();
        value.push(ALPHABET[index] as char);
    }
    value
}

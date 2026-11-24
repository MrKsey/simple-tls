use ::time::{Duration, OffsetDateTime};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
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

/// Describes the private key found in a PEM file for diagnostics.
///
/// rustls' `ring` provider only supports RSA 2048-4096, ECDSA P-256/P-384 and
/// Ed25519. Its error for anything else is a bare "failed to parse private key
/// as RSA, ECDSA, or EdDSA", which does not say what the file actually holds.
pub fn describe_private_key(pem: &[u8]) -> Option<String> {
    let (label, der) = pem_block(pem)?;
    let (tag, body, _) = der_tlv(&der)?;
    if tag != 0x30 {
        return Some(format!("{label} block"));
    }

    match label.as_str() {
        "EC PRIVATE KEY" => {
            let curve = sec1_curve(body)?;
            Some(format!(
                "{label}, {}",
                curve_description(
                    &curve,
                    matches!(curve.as_str(), "1.2.840.10045.3.1.7" | "1.3.132.0.34")
                )
            ))
        }
        "RSA PRIVATE KEY" => {
            let bits = rsa_bits(body)?;
            Some(format!("{label}, {bits}-bit RSA{}", rsa_range_note(bits)))
        }
        "PRIVATE KEY" => {
            let (algorithm, curve, key_der) = pkcs8_parts(body)?;
            let name = algorithm_name(&algorithm);
            match curve {
                Some(curve) => Some(format!(
                    "{label} (PKCS#8), {name}, {}",
                    curve_description(
                        &curve,
                        matches!(curve.as_str(), "1.2.840.10045.3.1.7" | "1.3.132.0.34")
                    )
                )),
                None => {
                    let rsa = if algorithm == "1.2.840.113549.1.1.1" {
                        rsa_bits_from_der(key_der)
                    } else {
                        None
                    };
                    match rsa {
                        Some(bits) => Some(format!(
                            "{label} (PKCS#8), {name}, {bits}-bit RSA{}",
                            rsa_range_note(bits)
                        )),
                        None => Some(format!("{label} (PKCS#8), {name}")),
                    }
                }
            }
        }
        _ => Some(format!("{label} block")),
    }
}

fn rsa_range_note(bits: usize) -> &'static str {
    if (2048..=4096).contains(&bits) {
        ""
    } else {
        " (ring requires 2048-4096 bits)"
    }
}

fn curve_description(oid: &str, supported: bool) -> String {
    let name = match oid {
        "1.2.840.10045.3.1.7" => "P-256 (prime256v1)",
        "1.3.132.0.34" => "P-384 (secp384r1)",
        "1.3.132.0.35" => "P-521 (secp521r1)",
        "1.3.132.0.10" => "secp256k1",
        "1.3.132.0.33" => "P-224 (secp224r1)",
        "1.2.840.10045.3.1.1" => "P-192 (prime192v1)",
        _ => return format!("curve with OID {oid}"),
    };
    if supported {
        format!("curve {name}, OID {oid}")
    } else {
        format!("curve {name}, OID {oid} (ring supports only P-256 and P-384)")
    }
}

fn algorithm_name(oid: &str) -> String {
    let name = match oid {
        "1.2.840.10045.2.1" => "id-ecPublicKey",
        "1.2.840.113549.1.1.1" => "rsaEncryption",
        "1.2.840.113549.1.1.10" => "RSASSA-PSS",
        "1.3.101.112" => "Ed25519",
        "1.3.101.113" => "Ed448",
        "1.2.840.10040.4.1" => "DSA",
        _ => return format!("algorithm with OID {oid}"),
    };
    format!("{name} (OID {oid})")
}

fn pem_block(pem: &[u8]) -> Option<(String, Vec<u8>)> {
    let text = std::str::from_utf8(pem).ok()?;
    let begin = text.find("-----BEGIN ")?;
    let label_start = begin + "-----BEGIN ".len();
    let label_end = label_start + text[label_start..].find("-----")?;
    let label = text[label_start..label_end].to_owned();
    let body_start = label_end + "-----".len();
    let end_marker = format!("-----END {label}-----");
    let body_end = body_start + text[body_start..].find(&end_marker)?;
    let body: String = text[body_start..body_end]
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let der = base64::prelude::BASE64_STANDARD.decode(body).ok()?;
    Some((label, der))
}

fn der_tlv(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *data.first()?;
    let (length, length_bytes) = der_length(data.get(1..)?)?;
    let start = 1 + length_bytes;
    let end = start.checked_add(length)?;
    Some((tag, data.get(start..end)?, data.get(end..)?))
}

fn der_length(data: &[u8]) -> Option<(usize, usize)> {
    let first = *data.first()?;
    if first < 0x80 {
        return Some((usize::from(first), 1));
    }
    let count = usize::from(first & 0x7f);
    if count == 0 || count > 4 {
        return None;
    }
    let mut length = 0usize;
    for byte in data.get(1..1 + count)? {
        length = (length << 8) | usize::from(*byte);
    }
    Some((length, 1 + count))
}

fn decode_oid(bytes: &[u8]) -> Option<String> {
    let first = u64::from(*bytes.first()?);
    let mut parts = vec![first / 40, first % 40];
    let mut value = 0u64;
    for byte in bytes.get(1..)? {
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            parts.push(value);
            value = 0;
        }
    }
    Some(
        parts
            .iter()
            .map(|part| part.to_string())
            .collect::<Vec<_>>()
            .join("."),
    )
}

/// Extracts the curve OID from a SEC1 `ECPrivateKey` structure.
fn sec1_curve(body: &[u8]) -> Option<String> {
    let (tag, _, rest) = der_tlv(body)?;
    if tag != 0x02 {
        return None;
    }
    let (tag, _, rest) = der_tlv(rest)?;
    if tag != 0x04 {
        return None;
    }
    let (tag, value, _) = der_tlv(rest)?;
    if tag != 0xa0 {
        return None;
    }
    let (tag, oid, _) = der_tlv(value)?;
    if tag != 0x06 {
        return None;
    }
    decode_oid(oid)
}

/// Extracts the algorithm, the optional curve OID and the private key bytes
/// from a PKCS#8 structure.
fn pkcs8_parts(body: &[u8]) -> Option<(String, Option<String>, &[u8])> {
    let (tag, _, rest) = der_tlv(body)?;
    if tag != 0x02 {
        return None;
    }
    let (tag, algorithm, rest) = der_tlv(rest)?;
    if tag != 0x30 {
        return None;
    }
    let (tag, key, _) = der_tlv(rest)?;
    if tag != 0x04 {
        return None;
    }
    let (tag, oid, parameters) = der_tlv(algorithm)?;
    if tag != 0x06 {
        return None;
    }
    let algorithm = decode_oid(oid)?;
    let curve = if parameters.is_empty() {
        None
    } else {
        match der_tlv(parameters) {
            Some((0x06, value, _)) => decode_oid(value),
            // RSA keys carry an explicit NULL here instead of a curve OID.
            _ => None,
        }
    };
    Some((algorithm, curve, key))
}

fn rsa_bits(body: &[u8]) -> Option<usize> {
    let (tag, _, rest) = der_tlv(body)?;
    if tag != 0x02 {
        return None;
    }
    let (tag, modulus, _) = der_tlv(rest)?;
    if tag != 0x02 {
        return None;
    }
    let modulus = modulus
        .iter()
        .skip_while(|byte| **byte == 0)
        .copied()
        .collect::<Vec<_>>();
    let leading = modulus
        .first()
        .map(|byte| 8 - byte.leading_zeros() as usize)
        .unwrap_or(0);
    Some(modulus.len().saturating_sub(1) * 8 + leading)
}

/// Like [`rsa_bits`], but for a complete PKCS#1 `RSAPrivateKey` DER blob,
/// which is what a PKCS#8 `OCTET STRING` holds.
fn rsa_bits_from_der(der: &[u8]) -> Option<usize> {
    let (tag, body, _) = der_tlv(der)?;
    if tag != 0x30 {
        return None;
    }
    rsa_bits(body)
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

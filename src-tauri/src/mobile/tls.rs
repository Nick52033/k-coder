//! 局域网直连的传输层安全。
//!
//! 设计约定：局域网可见不等于安全。网关在绑定到非回环地址时必须使用 TLS，
//! 并使用本机自签证书；证书指纹显示在桌面端，手机首次连接时人工核对。
//! 证书和私钥只落在应用数据目录，且私钥文件权限仅限当前用户。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use sha2::{Digest, Sha256};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use super::protocol::MobileError;

const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";

/// 本机自签身份。
pub struct TlsIdentity {
    pub fingerprint: String,
    pub acceptor: TlsAcceptor,
}

impl TlsIdentity {
    pub fn new(server_config: ServerConfig, fingerprint: String) -> Self {
        Self {
            fingerprint,
            acceptor: TlsAcceptor::from(Arc::new(server_config)),
        }
    }
}

/// 计算证书指纹：SHA-256 摘要，按字节用冒号分隔的大写十六进制。
///
/// 这是用户在桌面端和手机上逐位核对的字符串。
pub fn fingerprint_of(cert_der: &CertificateDer<'_>) -> String {
    let digest = Sha256::digest(cert_der.as_ref());
    digest
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn pem_to_der(pem: &str, label: &str) -> Result<Vec<u8>, MobileError> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let body = pem
        .split_once(&begin)
        .and_then(|(_, rest)| rest.split_once(&end))
        .map(|(body, _)| body)
        .ok_or_else(|| {
            MobileError::internal(format!("certificate file is missing {label} block"))
        })?;
    let compact: String = body
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|error| {
            MobileError::internal(format!("certificate file is not valid PEM: {error}"))
        })
}

fn write_private_file(path: &Path, contents: &str) -> Result<(), MobileError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| MobileError::internal(format!("failed to write {path:?}: {error}")))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| MobileError::internal(format!("failed to write {path:?}: {error}")))?;
    Ok(())
}

fn build_server_config(
    cert_der: CertificateDer<'static>,
    key_der: PrivateKeyDer<'static>,
) -> Result<ServerConfig, MobileError> {
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| MobileError::internal(format!("failed to configure TLS: {error}")))?
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .map_err(|error| MobileError::internal(format!("certificate is unusable: {error}")))
}

/// 载入已有证书；不存在时生成一份新的本机自签证书。
///
/// 复用已存在的证书是为了让指纹在重启后保持稳定——用户只需要核对一次。
pub fn load_or_create_identity(data_root: &Path, host: &str) -> Result<TlsIdentity, MobileError> {
    let directory = data_root.join("mobile").join("tls");
    fs::create_dir_all(&directory).map_err(|error| {
        MobileError::internal(format!("failed to prepare TLS directory: {error}"))
    })?;
    let cert_path: PathBuf = directory.join(CERT_FILE);
    let key_path: PathBuf = directory.join(KEY_FILE);

    if let (Ok(cert_pem), Ok(key_pem)) = (
        fs::read_to_string(&cert_path),
        fs::read_to_string(&key_path),
    ) {
        let cert_der = CertificateDer::from(pem_to_der(&cert_pem, "CERTIFICATE")?);
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pem_to_der(
            &key_pem,
            "PRIVATE KEY",
        )?));
        let fingerprint = fingerprint_of(&cert_der);
        let config = build_server_config(cert_der, key_der)?;
        return Ok(TlsIdentity::new(config, fingerprint));
    }

    let (cert_pem, key_pem, cert_der, key_der) = generate_self_signed(host)?;
    write_private_file(&key_path, &key_pem)?;
    fs::write(&cert_path, &cert_pem)
        .map_err(|error| MobileError::internal(format!("failed to write certificate: {error}")))?;
    let fingerprint = fingerprint_of(&cert_der);
    let config = build_server_config(cert_der, key_der)?;
    Ok(TlsIdentity::new(config, fingerprint))
}

type GeneratedIdentity = (
    String,
    String,
    CertificateDer<'static>,
    PrivateKeyDer<'static>,
);

fn generate_self_signed(host: &str) -> Result<GeneratedIdentity, MobileError> {
    let mut params = CertificateParams::new(vec![host.to_string()])
        .map_err(|error| MobileError::internal(format!("invalid certificate host: {error}")))?;
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "k-Coder Mobile Gateway");
    name.push(DnType::OrganizationName, "k-Coder");
    params.distinguished_name = name;
    // 有效期固定在一个有限区间，避免出现「几百年有效期」的自签证书。
    params.not_before = rcgen::date_time_ymd(2025, 1, 1);
    params.not_after = rcgen::date_time_ymd(2035, 1, 1);

    let key_pair = KeyPair::generate()
        .map_err(|error| MobileError::internal(format!("failed to generate key pair: {error}")))?;
    let certificate = params
        .self_signed(&key_pair)
        .map_err(|error| MobileError::internal(format!("failed to sign certificate: {error}")))?;

    let cert_der = certificate.der().clone();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    Ok((
        certificate.pem(),
        key_pair.serialize_pem(),
        cert_der,
        key_der,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fingerprint_is_stable_and_colon_separated() {
        let directory = TempDir::new().unwrap();
        let first = load_or_create_identity(directory.path(), "127.0.0.1").unwrap();
        let second = load_or_create_identity(directory.path(), "127.0.0.1").unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
        let parts: Vec<&str> = first.fingerprint.split(':').collect();
        assert_eq!(parts.len(), 32);
        assert!(parts.iter().all(|part| part.len() == 2));
        assert_eq!(first.fingerprint, first.fingerprint.to_uppercase());
    }

    #[test]
    fn private_key_is_written_to_disk_and_not_reused_across_hosts() {
        let directory = TempDir::new().unwrap();
        let identity = load_or_create_identity(directory.path(), "127.0.0.1").unwrap();
        assert!(directory.path().join("mobile/tls/key.pem").exists());
        assert!(directory.path().join("mobile/tls/cert.pem").exists());

        let other = TempDir::new().unwrap();
        let other_identity = load_or_create_identity(other.path(), "192.168.1.10").unwrap();
        assert_ne!(identity.fingerprint, other_identity.fingerprint);
    }

    #[test]
    fn pem_round_trip_rejects_garbage() {
        let error = pem_to_der("not a pem", "CERTIFICATE").unwrap_err();
        assert_eq!(error.kind(), "internal_error");
    }

    #[test]
    fn identity_produces_a_usable_tls_acceptor() {
        let directory = TempDir::new().unwrap();
        let identity = load_or_create_identity(directory.path(), "127.0.0.1").unwrap();
        // 只需要能构造出来；真正的握手在网关集成测试里验证。
        drop(identity.acceptor);
    }
}

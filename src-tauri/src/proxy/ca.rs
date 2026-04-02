//! Root CA generation, storage, and per-hostname certificate minting.

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub struct CertAuthority {
    ca_cert: rcgen::Certificate,
    ca_key: KeyPair,
    pub cert_path: PathBuf,
}

impl CertAuthority {
    fn load_existing(cert_path: &Path, key_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let cert_pem = fs::read_to_string(cert_path)?;
        let key_pem = fs::read_to_string(key_path)?;
        let ca_key = KeyPair::from_pem(&key_pem)?;
        let ca_params = CertificateParams::from_ca_cert_pem(&cert_pem)?;
        let ca_cert = ca_params.self_signed(&ca_key)?;
        Ok(Self {
            ca_cert,
            ca_key,
            cert_path: cert_path.to_path_buf(),
        })
    }

    /// Load existing CA or generate a new one. Stores in `dir/proxy-ca.crt` and `dir/proxy-ca.key`.
    pub fn load_or_create(dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let cert_path = dir.join("proxy-ca.crt");
        let key_path = dir.join("proxy-ca.key");

        let _ = fs::create_dir_all(dir);

        if cert_path.exists() && key_path.exists() {
            match Self::load_existing(&cert_path, &key_path) {
                Ok(ca) => return Ok(ca),
                Err(e) => {
                    println!("[fistbump] corrupt CA files, regenerating: {}", e);
                    let _ = fs::remove_file(&cert_path);
                    let _ = fs::remove_file(&key_path);
                }
            }
        }

        // Generate new CA
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Fistbump Local CA");
        dn.push(DnType::OrganizationName, "Fistbump");
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
        ];
        // Valid for 10 years
        params.not_before = time::OffsetDateTime::now_utc() - Duration::from_secs(86400);
        params.not_after =
            time::OffsetDateTime::now_utc() + Duration::from_secs(10 * 365 * 86400);

        let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let ca_cert = params.self_signed(&ca_key)?;

        // Save to disk
        fs::write(&cert_path, ca_cert.pem())?;
        fs::write(&key_path, ca_key.serialize_pem())?;

        // Restrict key file permissions on unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
        }

        println!("[fistbump] generated new root CA: {:?}", cert_path);

        Ok(Self {
            ca_cert,
            ca_key,
            cert_path,
        })
    }

    /// Mint a short-lived leaf certificate for a hostname, signed by this CA.
    pub fn mint_cert(
        &self,
        hostname: &str,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), Box<dyn std::error::Error>>
    {
        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, hostname);
        params.distinguished_name = dn;
        // Some hostnames (single chars, etc.) aren't valid DNS names for SAN.
        // Use the hostname if valid, otherwise omit SAN (CN still matches).
        if let Ok(dns_name) = hostname.try_into() {
            params.subject_alt_names = vec![SanType::DnsName(dns_name)];
        }
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        // Valid for 7 days, backdated 1 day
        params.not_before = time::OffsetDateTime::now_utc() - Duration::from_secs(86400);
        params.not_after = time::OffsetDateTime::now_utc() + Duration::from_secs(7 * 86400);

        let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let leaf_cert = params.signed_by(&leaf_key, &self.ca_cert, &self.ca_key)?;

        let cert_chain = vec![
            CertificateDer::from(leaf_cert.der().to_vec()),
            CertificateDer::from(self.ca_cert.der().to_vec()),
        ];
        let key_der =
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der().to_vec()));

        Ok((cert_chain, key_der))
    }
}

/// Wrapper for thread-safe sharing.
pub type SharedCA = Arc<CertAuthority>;

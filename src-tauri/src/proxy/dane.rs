//! DANE/TLSA validation — checks server certificates against TLSA records.

use sha2::{Digest, Sha256, Sha512};
use x509_parser::prelude::*;

use super::dns::TlsaRecord;

/// Validate a server certificate against TLSA records.
/// Returns true if any TLSA record matches (RFC 6698).
/// Returns false if no TLSA records are provided (caller decides whether to bypass).
pub fn validate(cert_der: &[u8], tlsa_records: &[TlsaRecord]) -> bool {
    if tlsa_records.is_empty() {
        return false;
    }

    for tlsa in tlsa_records {
        if matches_tlsa(cert_der, tlsa) {
            return true;
        }
    }

    false
}

fn matches_tlsa(cert_der: &[u8], tlsa: &TlsaRecord) -> bool {
    // Only support usage 3 (DANE-EE) for now — most common for self-hosted
    // Usage 1 (PKIX-EE) could be added later
    if tlsa.usage != 3 && tlsa.usage != 1 {
        return false;
    }

    let data_to_match = match tlsa.selector {
        0 => {
            // Full certificate DER
            cert_der.to_vec()
        }
        1 => {
            // SubjectPublicKeyInfo DER
            match X509Certificate::from_der(cert_der) {
                Ok((_, cert)) => cert.public_key().raw.to_vec(),
                Err(_) => return false,
            }
        }
        _ => return false,
    };

    match tlsa.matching_type {
        0 => {
            // Exact match
            data_to_match == tlsa.cert_data
        }
        1 => {
            // SHA-256
            let hash = Sha256::digest(&data_to_match);
            hash.as_slice() == tlsa.cert_data
        }
        2 => {
            // SHA-512
            let hash = Sha512::digest(&data_to_match);
            hash.as_slice() == tlsa.cert_data
        }
        _ => false,
    }
}

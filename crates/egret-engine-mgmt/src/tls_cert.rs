//! Self-signed TLS cert generation for jwt-mode coordinators. The coordinator
//! serves HTTPS with this cert; egret's query connector pins the PUBLIC cert
//! (`coordinator_cert`) to validate the server. SANs include the coordinator
//! Service host AND localhost/127.0.0.1 so a dev `kubectl port-forward` also
//! validates.

use rcgen::{CertificateParams, DistinguishedName, KeyPair, SanType};

/// Generate a self-signed cert for `dns_sans` (DNS names). Returns
/// `(cert_pem, key_pem)`. The keystore mounted to Trino is `cert_pem + key_pem`.
pub fn generate_self_signed(dns_sans: &[String]) -> Result<(String, String), String> {
    let mut params = CertificateParams::new(vec![]).map_err(|e| e.to_string())?;
    params.distinguished_name = DistinguishedName::new();
    params.subject_alt_names = dns_sans
        .iter()
        .map(|s| {
            Ok(SanType::DnsName(
                s.clone().try_into().map_err(|_| "bad SAN".to_string())?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    // Always cover loopback for the dev port-forward.
    params
        .subject_alt_names
        .push(SanType::IpAddress(std::net::IpAddr::from([127, 0, 0, 1])));
    params.subject_alt_names.push(SanType::DnsName(
        "localhost"
            .try_into()
            .map_err(|_| "localhost".to_string())?,
    ));
    let key = KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((cert.pem(), key.serialize_pem()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_cert_with_svc_and_localhost_sans() {
        let (cert_pem, key_pem) =
            generate_self_signed(&["rgr2-coordinator.egret-engines.svc".into()]).unwrap();
        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(key_pem.contains("PRIVATE KEY"));
        // Parse it back to confirm it is a valid cert and carries the SANs.
        let der = pem::parse(&cert_pem).unwrap();
        let (_, parsed) = x509_parser::parse_x509_certificate(der.contents()).unwrap();
        let san_str = format!("{:?}", parsed.subject_alternative_name().unwrap());
        assert!(san_str.contains("rgr2-coordinator.egret-engines.svc"));
        assert!(san_str.contains("localhost"));
    }
}

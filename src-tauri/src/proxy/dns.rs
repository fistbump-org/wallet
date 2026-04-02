//! DNS client — sends UDP queries to fbd's DNS server for name resolution.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

static FBD_DNS_PORT: AtomicU16 = AtomicU16::new(0);
const DNS_TIMEOUT: Duration = Duration::from_secs(3);

/// Set the DNS port based on the active network. Call once at startup.
pub fn set_port(port: u16) {
    FBD_DNS_PORT.store(port, Ordering::Relaxed);
}

// DNS record types
const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const TYPE_TLSA: u16 = 52;

const TYPE_CNAME: u16 = 5;

#[derive(Debug, Clone)]
pub struct ResolvedName {
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub cname: Option<String>,
    pub tlsa: Vec<TlsaRecord>,
}

#[derive(Debug, Clone)]
pub struct TlsaRecord {
    pub usage: u8,
    pub selector: u8,
    pub matching_type: u8,
    pub cert_data: Vec<u8>,
}

/// Build a minimal DNS query packet.
fn build_query(name: &str, qtype: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    // Header: ID=0xABCD, flags=0x0100 (RD=1), QDCOUNT=1
    buf.extend_from_slice(&[0xAB, 0xCD, 0x01, 0x00]);
    buf.extend_from_slice(&[0x00, 0x01]); // QDCOUNT
    buf.extend_from_slice(&[0x00, 0x00]); // ANCOUNT
    buf.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
    buf.extend_from_slice(&[0x00, 0x00]); // ARCOUNT

    // QNAME: encode labels
    for label in name.split('.') {
        if label.is_empty() {
            continue;
        }
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0); // root label

    // QTYPE and QCLASS (IN=1)
    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&[0x00, 0x01]);

    buf
}

/// Parse a DNS name from a response, handling compression pointers.
fn parse_name(data: &[u8], offset: &mut usize) -> Option<String> {
    let mut labels = Vec::new();
    let mut pos = *offset;
    let mut jumped = false;
    let mut jump_target = 0usize;
    let mut max_jumps = 10;

    loop {
        if pos >= data.len() || max_jumps == 0 {
            return None;
        }
        let len = data[pos] as usize;
        if len == 0 {
            if !jumped {
                *offset = pos + 1;
            }
            break;
        }
        if len & 0xC0 == 0xC0 {
            // Compression pointer
            if pos + 1 >= data.len() {
                return None;
            }
            let ptr = ((len & 0x3F) << 8) | data[pos + 1] as usize;
            if !jumped {
                jump_target = pos + 2;
            }
            pos = ptr;
            jumped = true;
            max_jumps -= 1;
            continue;
        }
        pos += 1;
        if pos + len > data.len() {
            return None;
        }
        labels.push(String::from_utf8_lossy(&data[pos..pos + len]).to_string());
        pos += len;
    }

    if jumped {
        *offset = jump_target;
    }

    Some(labels.join("."))
}

/// Parse an A record (4 bytes) into a dotted-decimal string.
fn parse_a(rdata: &[u8]) -> Option<String> {
    if rdata.len() < 4 {
        return None;
    }
    Some(format!("{}.{}.{}.{}", rdata[0], rdata[1], rdata[2], rdata[3]))
}

/// Parse an AAAA record (16 bytes) into an IPv6 string.
fn parse_aaaa(rdata: &[u8]) -> Option<String> {
    if rdata.len() < 16 {
        return None;
    }
    let mut groups = Vec::with_capacity(8);
    for i in 0..8 {
        groups.push(format!("{:x}", u16::from_be_bytes([rdata[i * 2], rdata[i * 2 + 1]])));
    }
    Some(groups.join(":"))
}

/// Parse a TLSA record from rdata.
fn parse_tlsa(rdata: &[u8]) -> Option<TlsaRecord> {
    if rdata.len() < 4 {
        return None;
    }
    Some(TlsaRecord {
        usage: rdata[0],
        selector: rdata[1],
        matching_type: rdata[2],
        cert_data: rdata[3..].to_vec(),
    })
}

/// Parse answer records from a DNS response.
fn parse_answers(data: &[u8], expected_type: u16) -> Vec<(u16, Vec<u8>)> {
    if data.len() < 12 {
        return vec![];
    }
    let ancount = u16::from_be_bytes([data[4], data[5]]) as usize;
    if ancount == 0 {
        return vec![];
    }

    // Skip header (12 bytes), then skip question section
    let mut offset = 12;
    // Skip QNAME
    parse_name(data, &mut offset);
    // Skip QTYPE + QCLASS
    offset += 4;

    let mut results = Vec::new();
    for _ in 0..ancount {
        if offset >= data.len() {
            break;
        }
        // Skip name
        parse_name(data, &mut offset);
        if offset + 10 > data.len() {
            break;
        }
        let rtype = u16::from_be_bytes([data[offset], data[offset + 1]]);
        // skip type(2) + class(2) + ttl(4)
        offset += 8;
        let rdlength = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + rdlength > data.len() {
            break;
        }
        if rtype == expected_type {
            results.push((rtype, data[offset..offset + rdlength].to_vec()));
        }
        offset += rdlength;
    }

    results
}

/// Extract the RCODE from a DNS response (lower 4 bits of byte 3).
fn dns_rcode(response: &[u8]) -> u8 {
    if response.len() < 4 { return 0; }
    response[3] & 0x0F
}

const RCODE_NOERROR: u8 = 0;
const RCODE_NXDOMAIN: u8 = 3;

/// Send a DNS query and return the raw response.
fn dns_query(name: &str, qtype: u16) -> Option<Vec<u8>> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(DNS_TIMEOUT)).ok()?;
    sock.set_write_timeout(Some(DNS_TIMEOUT)).ok()?;

    let query = build_query(name, qtype);
    sock.send_to(&query, format!("127.0.0.1:{}", FBD_DNS_PORT.load(Ordering::Relaxed))).ok()?;

    let mut buf = [0u8; 4096];
    let (len, _) = sock.recv_from(&mut buf).ok()?;
    Some(buf[..len].to_vec())
}

/// Result of a name resolution attempt.
#[derive(Debug)]
pub enum ResolveResult {
    /// Name resolved successfully with at least one address.
    Ok(ResolvedName),
    /// Name does not exist (NXDOMAIN).
    NotFound,
    /// Name exists but has no records at all.
    NoRecords,
    /// Name exists with records but no A/AAAA/CNAME (not browsable).
    NoWebsite,
    /// DNS query failed (server not reachable).
    DnsError,
}

/// Resolve a Fistbump name — queries fbd for A, AAAA, CNAME, and TLSA records.
pub fn resolve_full(name: &str) -> ResolveResult {
    let mut result = ResolvedName {
        ipv4: None,
        ipv6: None,
        cname: None,
        tlsa: Vec::new(),
    };

    // First, check if the name exists at all with a CNAME query.
    // NXDOMAIN means the name isn't registered.
    let cname_resp = match dns_query(name, TYPE_CNAME) {
        Some(r) => r,
        None => return ResolveResult::DnsError,
    };

    let rcode = dns_rcode(&cname_resp);
    if rcode == RCODE_NXDOMAIN {
        return ResolveResult::NotFound;
    }

    // Parse CNAME if present
    for (_, rdata) in parse_answers(&cname_resp, TYPE_CNAME) {
        let mut offset = 0;
        if let Some(target) = parse_name(&rdata, &mut offset) {
            let cleaned = target.strip_suffix('.').unwrap_or(&target).to_string();
            result.cname = Some(cleaned);
            break;
        }
    }

    // Query A record (CNAME-flattened by fbd DNS)
    if let Some(resp) = dns_query(name, TYPE_A) {
        for (_, rdata) in parse_answers(&resp, TYPE_A) {
            if let Some(ip) = parse_a(&rdata) {
                result.ipv4 = Some(ip);
                break;
            }
        }
    }

    // Query AAAA record
    if let Some(resp) = dns_query(name, TYPE_AAAA) {
        for (_, rdata) in parse_answers(&resp, TYPE_AAAA) {
            if let Some(ip) = parse_aaaa(&rdata) {
                result.ipv6 = Some(ip);
                break;
            }
        }
    }

    // If no CNAME, A, or AAAA — check if the name has any records at all.
    if result.cname.is_none() && result.ipv4.is_none() && result.ipv6.is_none() {
        // NOERROR with 0 answers on CNAME query means the name exists but
        // has no CNAME. If A/AAAA also returned nothing, check for other
        // record types (TLSA, TXT, etc.) to distinguish "no records" from
        // "no website records".
        let has_any = dns_query(name, TYPE_TLSA)
            .map(|r| !parse_answers(&r, TYPE_TLSA).is_empty())
            .unwrap_or(false);
        if rcode == RCODE_NOERROR && !has_any {
            return ResolveResult::NoRecords;
        }
        if rcode == RCODE_NOERROR && has_any {
            return ResolveResult::NoWebsite;
        }
        return ResolveResult::NoRecords;
    }

    // Query TLSA record (standard port prefix: _443._tcp.<name>)
    let tlsa_name = format!("_443._tcp.{}", name);
    if let Some(resp) = dns_query(&tlsa_name, TYPE_TLSA) {
        for (_, rdata) in parse_answers(&resp, TYPE_TLSA) {
            if let Some(tlsa) = parse_tlsa(&rdata) {
                result.tlsa.push(tlsa);
            }
        }
    }

    // Also try TLSA directly on the name
    if result.tlsa.is_empty() {
        if let Some(resp) = dns_query(name, TYPE_TLSA) {
            for (_, rdata) in parse_answers(&resp, TYPE_TLSA) {
                if let Some(tlsa) = parse_tlsa(&rdata) {
                    result.tlsa.push(tlsa);
                }
            }
        }
    }

    ResolveResult::Ok(result)
}

/// Legacy resolve — returns Some if address found, None otherwise.
pub fn resolve(name: &str) -> Option<ResolvedName> {
    match resolve_full(name) {
        ResolveResult::Ok(r) => Some(r),
        _ => None,
    }
}

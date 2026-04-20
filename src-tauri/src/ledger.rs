//! Ledger Stax integration for Fistbump.
//!
//! Mirrors the on-device Fistbump app at `~/Development/fistbump/ledger/`.
//! Two responsibilities:
//!
//! 1. **`cmd_get_account_xpub`** — fetch the BIP-32 pubkey + chain code at
//!    `m/44'/14159'/account'` from the device and serialize them as a
//!    standard `xpub…` string suitable for fbd's `createwallet`.
//! 2. **`cmd_sign_pstx`** — parse a v1 PSTX from fbd, walk its inputs, ask the
//!    Ledger to sign each one, and re-serialize the PSTX with sigs + pubkeys
//!    filled in. The wallet then submits the result via `broadcasttx`.

use std::collections::HashMap;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use bech32::Hrp;
use hidapi::{HidApi, HidDevice};
use sha2::{Digest, Sha256};

/// All Ledger HID operations run on a single dedicated thread.
///
/// macOS IOKit (`IOHIDManagerSetDeviceMatchingMultiple`,
/// `IOHIDDeviceScheduleWithRunLoop`) schedules HID notification sources on
/// the *calling thread's* CFRunLoop. When two tokio worker threads ever
/// touch the same `HidApi` (or `HidDevice`s derived from it), CFRunLoop's
/// internal info gets mutated concurrently and the next call PAC-traps in
/// `__CFCheckCFInfoPACSignature`. Serializing through a single thread —
/// not just a mutex — gives IOKit a stable run loop to live on.
type Job = Box<dyn FnOnce(&mut HidApi) + Send + 'static>;

static JOB_TX: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();

fn dispatch<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce(&mut HidApi) -> Result<R, String> + Send + 'static,
    R: Send + 'static,
{
    let tx_mutex = JOB_TX.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        thread::Builder::new()
            .name("fistbump-ledger-hid".into())
            .spawn(move || hid_thread_main(rx))
            .expect("spawn ledger HID thread");
        Mutex::new(tx)
    });
    let (rtx, rrx) = sync_channel::<Result<R, String>>(1);
    let job: Job = Box::new(move |api: &mut HidApi| {
        let _ = rtx.send(f(api));
    });
    {
        let tx = tx_mutex.lock().map_err(|e| format!("ledger dispatch lock: {e}"))?;
        tx.send(job).map_err(|_| "ledger HID thread died".to_string())?;
    }
    rrx.recv().map_err(|e| format!("ledger HID reply: {e}"))?
}

fn hid_thread_main(rx: Receiver<Job>) {
    let mut api = match HidApi::new() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("fistbump-ledger-hid: HidApi init failed: {e}");
            return;
        }
    };
    while let Ok(job) = rx.recv() {
        job(&mut api);
    }
}

const LEDGER_VID: u16 = 0x2c97;
const SLIP44_FBC: u32 = 14159;
const HARDENED: u32 = 0x8000_0000;

const APDU_CLA: u8 = 0xe0;
const INS_GET_PUBKEY: u8 = 0x05;
const INS_SIGN_TX: u8 = 0x06;
const P2_MORE: u8 = 0x80;
const P2_LAST: u8 = 0x00;

const HID_CID: u16 = 0x0101;
const HID_TAG_APDU: u8 = 0x05;
const HID_FRAME_LEN: usize = 64;

const SW_OK: u16 = 0x9000;

pub struct Ledger {
    device: HidDevice,
}

impl Ledger {
    /// Open the Ledger using the given `HidApi`. **Must run on the dedicated
    /// HID thread** — see `dispatch`. Refreshes the device list each call so
    /// a newly-plugged Ledger is found.
    pub fn open_with(api: &mut HidApi) -> Result<Self, String> {
        api.refresh_devices()
            .map_err(|e| format!("hidapi refresh failed: {e}"))?;
        // Ledger devices expose several HID interfaces (raw APDU, U2F, keyboard).
        // The raw APDU one has interface_number == 0 OR usage_page == 0xFFA0.
        // Without this filter we'd happily open e.g. the U2F interface and
        // hang forever on the read because it doesn't speak our framing.
        for info in api.device_list() {
            if info.vendor_id() != LEDGER_VID {
                continue;
            }
            let is_apdu_iface = info.interface_number() == 0 || info.usage_page() == 0xFFA0;
            if !is_apdu_iface {
                continue;
            }
            let dev = api
                .open_path(info.path())
                .map_err(|e| format!("open Ledger device failed: {e}"))?;
            return Ok(Self { device: dev });
        }
        Err("no Ledger device found — connect & unlock the device, then open Fistbump".into())
    }

    fn exchange(&self, apdu: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
        // Outbound: chunk APDU into Ledger HID frames.
        // Frame: cid(2 BE) || tag(1) || seq(2 BE) || payload.
        // First payload is prefixed with the total APDU length (2 BE).
        let total_len = u16::try_from(apdu.len()).map_err(|_| "apdu too long")?;
        let mut payload = Vec::with_capacity(2 + apdu.len());
        payload.extend_from_slice(&total_len.to_be_bytes());
        payload.extend_from_slice(apdu);

        let mut seq: u16 = 0;
        let mut offset = 0;
        while offset < payload.len() {
            let chunk_size = (payload.len() - offset).min(HID_FRAME_LEN - 5);
            // [report_id, frame...] — hidapi-rs expects the report ID prefix.
            let mut report = [0u8; HID_FRAME_LEN + 1];
            report[1..3].copy_from_slice(&HID_CID.to_be_bytes());
            report[3] = HID_TAG_APDU;
            report[4..6].copy_from_slice(&seq.to_be_bytes());
            report[6..6 + chunk_size].copy_from_slice(&payload[offset..offset + chunk_size]);
            self.device
                .write(&report)
                .map_err(|e| format!("HID write failed: {e}"))?;
            offset += chunk_size;
            seq = seq.checked_add(1).ok_or("HID seq overflow")?;
        }

        // Inbound: re-assemble response frames.
        let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let mut response = Vec::new();
        let mut expected: Option<usize> = None;
        let mut seq: u16 = 0;
        loop {
            let mut buf = [0u8; HID_FRAME_LEN];
            let n = self
                .device
                .read_timeout(&mut buf, timeout_ms)
                .map_err(|e| format!("HID read failed: {e}"))?;
            if n < 5 {
                return Err(format!("short HID frame ({n} bytes)"));
            }
            let cid = u16::from_be_bytes([buf[0], buf[1]]);
            if cid != HID_CID || buf[2] != HID_TAG_APDU {
                return Err(format!("bad HID header cid={cid:04x} tag={:02x}", buf[2]));
            }
            let got_seq = u16::from_be_bytes([buf[3], buf[4]]);
            if got_seq != seq {
                return Err(format!("HID seq mismatch (expected {seq}, got {got_seq})"));
            }
            let mut frame_payload = &buf[5..n];
            if seq == 0 {
                if frame_payload.len() < 2 {
                    return Err("first HID frame missing total length".into());
                }
                expected = Some(u16::from_be_bytes([frame_payload[0], frame_payload[1]]) as usize);
                frame_payload = &frame_payload[2..];
            }
            response.extend_from_slice(frame_payload);
            seq = seq.checked_add(1).ok_or("HID seq overflow on read")?;
            if let Some(t) = expected {
                if response.len() >= t {
                    response.truncate(t);
                    break;
                }
            }
        }

        if response.len() < 2 {
            return Err(format!("APDU response too short ({} bytes)", response.len()));
        }
        let sw_lo = response.pop().unwrap();
        let sw_hi = response.pop().unwrap();
        let sw = u16::from_be_bytes([sw_hi, sw_lo]);
        if sw != SW_OK {
            return Err(format!("device returned SW=0x{sw:04x}"));
        }
        Ok(response)
    }

    /// GET_PUBKEY at the given BIP-32 path.
    /// Returns (compressed pubkey 33 B, chain code 32 B, bech32 address).
    pub fn get_pubkey(
        &self,
        path: &[u32],
        display: bool,
    ) -> Result<(Vec<u8>, Vec<u8>, String), String> {
        let path_data = encode_bip32_path(path);
        let p1 = if display { 1 } else { 0 };
        let apdu = build_apdu(INS_GET_PUBKEY, p1, 0, &path_data);
        let resp = self.exchange(&apdu, Duration::from_secs(60))?;
        if resp.len() < 1 + 33 + 1 + 32 + 1 {
            return Err(format!("get_pubkey response too short: {} bytes", resp.len()));
        }
        let pubkey_len = resp[0] as usize;
        if pubkey_len != 33 {
            return Err(format!("expected 33B pubkey, got {pubkey_len}"));
        }
        let pubkey = resp[1..34].to_vec();
        let cc_len = resp[34] as usize;
        if cc_len != 32 {
            return Err(format!("expected 32B chain code, got {cc_len}"));
        }
        let chain_code = resp[35..67].to_vec();
        let addr_len = resp[67] as usize;
        let addr_end = 68 + addr_len;
        if resp.len() < addr_end {
            return Err("get_pubkey response truncated at address".into());
        }
        let address = String::from_utf8(resp[68..addr_end].to_vec())
            .map_err(|e| format!("address utf8: {e}"))?;
        Ok((pubkey, chain_code, address))
    }

    /// SIGN_TX over chunked APDUs.
    /// Returns (sig 64 B compact, sighash type byte, compressed pubkey 33 B).
    /// Receiving the pubkey here avoids a follow-up GET_PUBKEY APDU, which on
    /// Stax was hanging the device while the post-sign NBGL status screen was up.
    pub fn sign_tx(
        &self,
        path: &[u32],
        input_index: u32,
        value: u64,
        sighash_type: u8,
        unsigned_tx: &[u8],
    ) -> Result<(Vec<u8>, u8, Vec<u8>), String> {
        // Chunk 0: BIP-32 path.
        let path_data = encode_bip32_path(path);
        let apdu = build_apdu(INS_SIGN_TX, 0, P2_MORE, &path_data);
        self.exchange(&apdu, Duration::from_secs(10))?;

        // Chunks 1..=3: metadata + tx bytes.
        let mut payload = Vec::with_capacity(13 + unsigned_tx.len());
        payload.extend_from_slice(&input_index.to_be_bytes());
        payload.extend_from_slice(&value.to_le_bytes());
        payload.push(sighash_type);
        payload.extend_from_slice(unsigned_tx);

        const MAX_CHUNK: usize = 240;
        let total_chunks = payload.len().div_ceil(MAX_CHUNK);
        if total_chunks == 0 || total_chunks > 3 {
            return Err(format!("payload requires {total_chunks} chunks, expected 1..=3"));
        }
        let mut last_response = Vec::new();
        for i in 0..total_chunks {
            let start = i * MAX_CHUNK;
            let end = (start + MAX_CHUNK).min(payload.len());
            let p1 = (i + 1) as u8;
            let p2 = if i + 1 == total_chunks { P2_LAST } else { P2_MORE };
            let apdu = build_apdu(INS_SIGN_TX, p1, p2, &payload[start..end]);
            // The final chunk waits on user approval — generous timeout.
            let timeout = if p2 == P2_LAST {
                Duration::from_secs(120)
            } else {
                Duration::from_secs(10)
            };
            last_response = self.exchange(&apdu, timeout)?;
        }

        // Layout: sig(64) || sighash(1) || pk_len(1) || pk(pk_len).
        if last_response.len() < 67 {
            return Err(format!(
                "SIGN_TX response too short ({} bytes)",
                last_response.len()
            ));
        }
        let pk_len = last_response[65] as usize;
        if pk_len != 33 || last_response.len() != 66 + pk_len {
            return Err(format!(
                "SIGN_TX pubkey malformed: pk_len={pk_len}, total={}",
                last_response.len()
            ));
        }
        let pubkey = last_response[66..66 + pk_len].to_vec();
        let sighash = last_response[64];
        last_response.truncate(64);
        Ok((last_response, sighash, pubkey))
    }
}

fn encode_bip32_path(components: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + components.len() * 4);
    out.push(components.len() as u8);
    for c in components {
        out.extend_from_slice(&c.to_be_bytes());
    }
    out
}

fn build_apdu(ins: u8, p1: u8, p2: u8, data: &[u8]) -> Vec<u8> {
    let mut apdu = Vec::with_capacity(5 + data.len());
    apdu.extend_from_slice(&[APDU_CLA, ins, p1, p2, data.len() as u8]);
    apdu.extend_from_slice(data);
    apdu
}

/// Build a standard BIP-32 `xpub…` string from raw pubkey + chain code at
/// `m/44'/14159'/account'`.
///
/// fbd's `ExtendedPublicKey.deserialize` doesn't validate the parent
/// fingerprint against the actual parent, so we ship 0 and avoid an extra
/// device round-trip.
pub fn assemble_xpub(account: u32, pubkey: &[u8], chain_code: &[u8]) -> Result<String, String> {
    if pubkey.len() != 33 {
        return Err(format!("pubkey must be 33B, got {}", pubkey.len()));
    }
    if chain_code.len() != 32 {
        return Err(format!("chain code must be 32B, got {}", chain_code.len()));
    }
    let mut payload = Vec::with_capacity(78);
    payload.extend_from_slice(&[0x04, 0x88, 0xB2, 0x1E]); // version
    payload.push(3); // depth: m/44'/14159'/account' is depth 3
    payload.extend_from_slice(&[0u8; 4]); // parent fingerprint (unverified)
    payload.extend_from_slice(&(HARDENED | account).to_be_bytes()); // child index
    payload.extend_from_slice(chain_code);
    payload.extend_from_slice(pubkey);
    let h1 = Sha256::digest(&payload);
    let h2 = Sha256::digest(h1);
    payload.extend_from_slice(&h2[..4]);
    Ok(bs58::encode(payload).into_string())
}

/// Encode a Fistbump P2WPKH address as bech32 — used to look up paths in
/// fbd's listaddresses map.
pub fn encode_bech32_address(network: &str, version: u8, hash: &[u8]) -> Result<String, String> {
    let hrp_str = match network {
        "main" | "mainnet" => "fb",
        "testnet" => "ft",
        "regtest" => "fr",
        "simnet" => "fs",
        other => return Err(format!("unknown network: {other}")),
    };
    if version != 0 {
        return Err(format!("only witness version 0 supported, got {version}"));
    }
    let hrp = Hrp::parse(hrp_str).map_err(|e| format!("hrp parse: {e}"))?;
    bech32::segwit::encode_v0(hrp, hash).map_err(|e| format!("bech32 encode: {e}"))
}

// ─── PSTX (V1) parser/serializer ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PstxCoin {
    pub value: u64,
    pub address_version: u8,
    pub address_hash: Vec<u8>,
    pub outpoint_hash: [u8; 32],
    pub outpoint_index: u32,
}

#[derive(Debug, Clone)]
pub struct PstxV1 {
    pub tx_bytes: Vec<u8>,
    pub coins: Vec<PstxCoin>,
}

impl PstxV1 {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        if data.len() < 6 || data[0] != 0x01 {
            return Err("not a v1 PSTX".into());
        }
        let mut r = Cursor::new(data, 1);
        let tx_len = r.read_u32_le()? as usize;
        let tx_bytes = r.read_bytes(tx_len)?.to_vec();
        let coin_count = r.read_u16_le()? as usize;
        let mut coins = Vec::with_capacity(coin_count);
        for _ in 0..coin_count {
            let value = r.read_u64_le()?;
            let address_version = r.read_u8()?;
            let hash_len = r.read_u8()? as usize;
            let address_hash = r.read_bytes(hash_len)?.to_vec();
            let outpoint_hash = r.read_array::<32>()?;
            let outpoint_index = r.read_u32_le()?;
            coins.push(PstxCoin {
                value,
                address_version,
                address_hash,
                outpoint_hash,
                outpoint_index,
            });
        }
        Ok(PstxV1 { tx_bytes, coins })
    }

    /// Re-serialize, replacing the per-input section with our P2WPKH
    /// `(sig_with_sighash_byte, compressed_pubkey)` pairs.
    pub fn serialize_p2wpkh(&self, inputs: &[(Vec<u8>, Vec<u8>)]) -> Result<Vec<u8>, String> {
        if inputs.len() != self.coins.len() {
            return Err(format!(
                "expected {} input slots, got {}",
                self.coins.len(),
                inputs.len()
            ));
        }
        let mut out = Vec::new();
        out.push(0x01);
        out.extend_from_slice(&(self.tx_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.tx_bytes);
        out.extend_from_slice(&(self.coins.len() as u16).to_le_bytes());
        for coin in &self.coins {
            out.extend_from_slice(&coin.value.to_le_bytes());
            out.push(coin.address_version);
            out.push(coin.address_hash.len() as u8);
            out.extend_from_slice(&coin.address_hash);
            out.extend_from_slice(&coin.outpoint_hash);
            out.extend_from_slice(&coin.outpoint_index.to_le_bytes());
        }
        for (sig, pk) in inputs {
            out.push(0x00); // P2WPKH input type
            out.push(sig.len() as u8);
            out.extend_from_slice(sig);
            if pk.len() == 33 {
                out.push(33);
                out.extend_from_slice(pk);
            } else {
                out.push(0);
            }
        }
        Ok(out)
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }
    fn need(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).ok_or("cursor overflow")?;
        if end > self.data.len() {
            return Err(format!(
                "buffer underflow at pos {}: need {n}, have {}",
                self.pos,
                self.data.len() - self.pos
            ));
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn read_u8(&mut self) -> Result<u8, String> {
        Ok(self.need(1)?[0])
    }
    fn read_u16_le(&mut self) -> Result<u16, String> {
        let b = self.need(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn read_u32_le(&mut self) -> Result<u32, String> {
        let b = self.need(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn read_u64_le(&mut self) -> Result<u64, String> {
        let b = self.need(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.need(n)
    }
    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let s = self.need(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }
}

// ─── Tauri command bodies (sync — wrap in spawn_blocking from lib.rs) ───────

pub fn cmd_get_account_xpub(account: u32) -> Result<String, String> {
    dispatch(move |api: &mut HidApi| -> Result<String, String> {
        let ledger = Ledger::open_with(api)?;
        let path = vec![HARDENED | 44, HARDENED | SLIP44_FBC, HARDENED | account];
        let (pubkey, chain_code, _) = ledger.get_pubkey(&path, false)?;
        assemble_xpub(account, &pubkey, &chain_code)
    })
}

pub fn cmd_sign_pstx(
    pstx_hex: &str,
    network: &str,
    address_to_path: &HashMap<String, String>,
) -> Result<String, String> {
    // Clone borrowed inputs so the closure can be `'static` for the dispatch thread.
    let pstx_hex = pstx_hex.to_string();
    let network = network.to_string();
    let address_to_path = address_to_path.clone();
    dispatch(move |api: &mut HidApi| -> Result<String, String> {
        let pstx_bytes = hex::decode(pstx_hex.trim()).map_err(|e| format!("hex decode: {e}"))?;
        let pstx = PstxV1::parse(&pstx_bytes)?;
        let ledger = Ledger::open_with(api)?;
        let mut input_pairs = Vec::with_capacity(pstx.coins.len());
        for (i, coin) in pstx.coins.iter().enumerate() {
            let addr = encode_bech32_address(&network, coin.address_version, &coin.address_hash)?;
            let path_str = address_to_path
                .get(&addr)
                .ok_or_else(|| format!("no derivation path for input {i} address {addr}"))?;
            let path = parse_path(path_str)?;
            let (sig, sighash_byte, pubkey) =
                ledger.sign_tx(&path, i as u32, coin.value, 0x01, &pstx.tx_bytes)?;
            let mut sig_with_type = sig;
            sig_with_type.push(sighash_byte);
            input_pairs.push((sig_with_type, pubkey));
        }
        let signed = pstx.serialize_p2wpkh(&input_pairs)?;
        Ok(hex::encode(signed))
    })
}

fn parse_path(path: &str) -> Result<Vec<u32>, String> {
    let trimmed = path.trim_start_matches("m/").trim_start_matches('/');
    let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        let hardened = p.ends_with('\'');
        let n: u32 = p
            .trim_end_matches('\'')
            .parse()
            .map_err(|e| format!("path component {p:?}: {e}"))?;
        out.push(if hardened { HARDENED | n } else { n });
    }
    Ok(out)
}

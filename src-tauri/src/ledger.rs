//! Ledger Stax integration for Fistbump.
//!
//! Mirrors the on-device Fistbump app at `~/Development/fistbump/ledger/`.
//! Two responsibilities:
//!
//! 1. **`cmd_get_account_xpub`** — fetch the BIP-32 pubkey + chain code at
//!    `m/44'/14159'/account'` from the device and serialize them as a
//!    standard `xpub…` string suitable for fbd's `createwallet`.
//! 2. **`cmd_sign_pstx`** — parse a v1 PSTX from fbd, walk its inputs, ask
//!    the Ledger to sign each one, and re-serialize the PSTX with sigs +
//!    pubkeys filled in. The wallet then submits the result via
//!    `broadcasttx`.
//!
//! Transports (USB HID today, BLE coming) implement the `LedgerTransport`
//! trait in their own submodules; `Ledger<T>` wraps one and speaks the
//! APDU layer on top.

pub mod ble;
#[cfg(desktop)]
pub mod hid;

use std::collections::HashMap;
use std::time::Duration;

use bech32::Hrp;
use sha2::{Digest, Sha256};

use ble::BleTransport;
#[cfg(desktop)]
use hid::HidTransport;

const SLIP44_FBC: u32 = 14159;
const HARDENED: u32 = 0x8000_0000;

const APDU_CLA: u8 = 0xe0;
const INS_GET_PUBKEY: u8 = 0x05;
const INS_SIGN_TX: u8 = 0x06;
const INS_SIGN_MESSAGE: u8 = 0x07;

// SIGN_TX sub-op P1 selectors (match on-device main.rs).
const P1_SIGN_BEGIN: u8 = 0x00;
const P1_SIGN_ADD_INPUT: u8 = 0x01;
const P1_SIGN_ADD_OUTPUT: u8 = 0x02;
const P1_SIGN_REVIEW: u8 = 0x03;
const P1_SIGN_INPUT: u8 = 0x04;

// P2 chunking for ADD_OUTPUT.
const P2_MORE: u8 = 0x80;
const P2_LAST: u8 = 0x00;

/// Script kind byte in ADD_INPUT. P2WPKH is implemented; P2WSH (multisig)
/// is reserved and will be wired up once the on-device script parser
/// lands.
const SCRIPT_KIND_P2WPKH: u8 = 0x00;

/// Max payload size per APDU data field (sized well under the BLE MTU
/// and HID frame accumulator limits; everything longer gets chunked).
const MAX_APDU_DATA: usize = 240;

const SW_OK: u16 = 0x9000;

// ─── Transport abstraction ─────────────────────────────────────────────────

/// Wire-level link to a Ledger device. Implementations own a physical
/// transport (USB HID, BLE, …) and hide its framing behind one APDU
/// request/response call. The 2-byte status word is validated and stripped
/// by the transport — callers only see response data.
pub trait LedgerTransport {
    fn exchange(&self, apdu: &[u8], timeout: Duration) -> Result<Vec<u8>, String>;
}

// ─── Ledger APDU layer ─────────────────────────────────────────────────────

pub struct Ledger<T: LedgerTransport> {
    transport: T,
}

impl<T: LedgerTransport> Ledger<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
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
        let resp = self.transport.exchange(&apdu, Duration::from_secs(60))?;
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

    /// Stream a PSBT-style signing session to the device and collect a
    /// signature for each input.
    ///
    /// Sequence: `BEGIN` → one `ADD_INPUT` per input → one `ADD_OUTPUT`
    /// per output (chunked if needed) → `REVIEW` (user approves on
    /// device) → one `SIGN_INPUT` per input → done. The user taps "Hold
    /// to sign" exactly once regardless of input count. The device sums
    /// input and output values itself, so the fee the user sees is the
    /// real fee even for multi-input txs.
    pub fn sign_pstx(
        &self,
        tx_bytes: &[u8],
        inputs: &[SignInput],
    ) -> Result<Vec<SignResult>, String> {
        let parsed = parse_unsigned_tx(tx_bytes)?;
        if parsed.inputs.len() != inputs.len() {
            return Err(format!(
                "input count mismatch: tx has {}, caller provided {}",
                parsed.inputs.len(),
                inputs.len()
            ));
        }

        self.send_begin(
            parsed.version,
            inputs.len() as u64,
            parsed.outputs.len() as u64,
            parsed.locktime,
        )?;
        for (input_tx, meta) in parsed.inputs.iter().zip(inputs.iter()) {
            self.send_add_input(input_tx, meta)?;
        }
        for output_slice in &parsed.outputs {
            self.send_add_output(output_slice)?;
        }
        self.send_review()?;

        let mut results = Vec::with_capacity(inputs.len());
        for i in 0..inputs.len() {
            results.push(self.send_sign_input(i as u32)?);
        }
        Ok(results)
    }

    fn send_begin(&self, version: u32, n_in: u64, n_out: u64, locktime: u32) -> Result<(), String> {
        let mut payload = Vec::with_capacity(18);
        payload.extend_from_slice(&version.to_le_bytes());
        write_varint(&mut payload, n_in);
        write_varint(&mut payload, n_out);
        payload.extend_from_slice(&locktime.to_le_bytes());
        let apdu = build_apdu(INS_SIGN_TX, P1_SIGN_BEGIN, 0, &payload);
        self.transport.exchange(&apdu, Duration::from_secs(10))?;
        Ok(())
    }

    fn send_add_input(&self, input: &TxInputRef, meta: &SignInput) -> Result<(), String> {
        let mut payload = Vec::with_capacity(64 + meta.path.len() * 4);
        payload.extend_from_slice(&input.prev_hash);
        payload.extend_from_slice(&input.prev_index.to_le_bytes());
        payload.extend_from_slice(&input.sequence.to_le_bytes());
        payload.extend_from_slice(&meta.value.to_le_bytes());
        payload.push(meta.sighash_type);
        if meta.path.is_empty() || meta.path.len() > 10 {
            return Err(format!("path length {} out of range", meta.path.len()));
        }
        payload.push(meta.path.len() as u8);
        for word in &meta.path {
            payload.extend_from_slice(&word.to_be_bytes());
        }
        payload.push(SCRIPT_KIND_P2WPKH);
        let apdu = build_apdu(INS_SIGN_TX, P1_SIGN_ADD_INPUT, 0, &payload);
        self.transport.exchange(&apdu, Duration::from_secs(10))?;
        Ok(())
    }

    fn send_add_output(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() {
            return Err("output bytes empty".into());
        }
        let mut offset = 0;
        while offset < bytes.len() {
            let end = (offset + MAX_APDU_DATA).min(bytes.len());
            let p2 = if end == bytes.len() { P2_LAST } else { P2_MORE };
            let apdu = build_apdu(INS_SIGN_TX, P1_SIGN_ADD_OUTPUT, p2, &bytes[offset..end]);
            self.transport.exchange(&apdu, Duration::from_secs(10))?;
            offset = end;
        }
        Ok(())
    }

    fn send_review(&self) -> Result<(), String> {
        // Generous timeout: waits on the user's "Hold to sign" (or reject).
        let apdu = build_apdu(INS_SIGN_TX, P1_SIGN_REVIEW, 0, &[]);
        self.transport.exchange(&apdu, Duration::from_secs(180))?;
        Ok(())
    }

    /// Sign a message with the key at `path`, matching
    /// `fbd::ECDSASigner::signRecoverable`: the device prepends
    /// `"fistbump signed message:\n"`, BLAKE2b-256 hashes, shows the
    /// preview for user approval, then returns the signature. The final
    /// on-wire format is the 65-byte `recovery_id || compact_sig`
    /// recoverable signature fbd's `verifymessage` expects.
    pub fn sign_message(&self, path: &[u32], message: &[u8]) -> Result<Vec<u8>, String> {
        if message.len() > 512 {
            return Err(format!("message too long ({} bytes, max 512)", message.len()));
        }
        let mut payload = Vec::with_capacity(1 + path.len() * 4 + 2 + message.len());
        if path.is_empty() || path.len() > 10 {
            return Err(format!("path length {} out of range", path.len()));
        }
        payload.push(path.len() as u8);
        for w in path {
            payload.extend_from_slice(&w.to_be_bytes());
        }
        payload.extend_from_slice(&(message.len() as u16).to_le_bytes());
        payload.extend_from_slice(message);

        let apdu = build_apdu(INS_SIGN_MESSAGE, 0, 0, &payload);
        // Generous timeout — waits on the user's "Hold to sign".
        let resp = self.transport.exchange(&apdu, Duration::from_secs(180))?;
        // Response layout: recovery_id(1) || sig_compact(64) || pubkey_compressed(33) = 98 B.
        if resp.len() != 98 {
            return Err(format!(
                "SIGN_MESSAGE response wrong length: {} (expected 98)",
                resp.len()
            ));
        }
        let rec_id = resp[0];
        if rec_id > 3 {
            return Err(format!("SIGN_MESSAGE invalid recovery_id {rec_id}"));
        }
        // fbd's 65-byte format: [recovery_id, r(32), s(32)]
        let mut sig = Vec::with_capacity(65);
        sig.push(rec_id);
        sig.extend_from_slice(&resp[1..65]);
        Ok(sig)
    }

    fn send_sign_input(&self, index: u32) -> Result<SignResult, String> {
        let apdu = build_apdu(INS_SIGN_TX, P1_SIGN_INPUT, 0, &index.to_be_bytes());
        let resp = self.transport.exchange(&apdu, Duration::from_secs(30))?;
        // Layout: sig(64) || sighash(1) || pk_len(1) || pk(pk_len)
        if resp.len() < 67 {
            return Err(format!("SIGN_INPUT response too short ({} bytes)", resp.len()));
        }
        let sighash = resp[64];
        let pk_len = resp[65] as usize;
        if pk_len != 33 || resp.len() != 66 + pk_len {
            return Err(format!(
                "SIGN_INPUT pubkey malformed: pk_len={pk_len}, total={}",
                resp.len()
            ));
        }
        let sig = resp[..64].to_vec();
        let pubkey = resp[66..66 + pk_len].to_vec();
        Ok(SignResult {
            sig,
            sighash_byte: sighash,
            pubkey,
        })
    }
}

/// Per-input signing metadata the wallet hands to `sign_pstx`.
pub struct SignInput {
    pub path: Vec<u32>,
    pub value: u64,
    pub sighash_type: u8,
}

/// What the device returns for each input after the user approves the tx.
pub struct SignResult {
    pub sig: Vec<u8>,          // 64 B compact
    pub sighash_byte: u8,
    pub pubkey: Vec<u8>,       // 33 B compressed
}

// ─── Minimal unsigned-tx parser ────────────────────────────────────────────

/// A reference into `tx_bytes` for one input's on-wire fields. The device's
/// ADD_INPUT payload wants (prev_hash, prev_index, sequence) — we just lift
/// those out of the serialized tx.
struct TxInputRef {
    prev_hash: [u8; 32],
    prev_index: u32,
    sequence: u32,
}

struct ParsedTx<'a> {
    version: u32,
    locktime: u32,
    inputs: Vec<TxInputRef>,
    /// Byte slices (one per output) into the source `tx_bytes`. Forwarded
    /// verbatim to the device as ADD_OUTPUT payload — the on-device parser
    /// understands the same layout, so we don't re-serialize.
    outputs: Vec<&'a [u8]>,
}

/// Parse an unsigned Fistbump tx just far enough to extract the fields the
/// device needs. The layout (mirroring `fbd/Sources/Protocol/Transaction.swift`):
///
/// ```text
/// version(4 LE) || varint(n_in) || n_in × input(36+4 LE) ||
///   varint(n_out) || n_out × output(variable) || locktime(4 LE)
/// ```
///
/// Each `output` is `value(8) || addr_version(1) || addr_len(1) || addr_hash(·) ||
/// covenant_kind(1) || varint(n_items) || n_items × (varint_len || item_bytes)`.
fn parse_unsigned_tx(buf: &[u8]) -> Result<ParsedTx<'_>, String> {
    let mut r = TxReader::new(buf);
    let version = r.read_u32_le()?;
    let n_in = r.read_varint()? as usize;
    if n_in == 0 || n_in > 256 {
        return Err(format!("unreasonable input count {n_in}"));
    }
    let mut inputs = Vec::with_capacity(n_in);
    for _ in 0..n_in {
        let prev_hash = r.read_array::<32>()?;
        let prev_index = r.read_u32_le()?;
        let sequence = r.read_u32_le()?;
        inputs.push(TxInputRef {
            prev_hash,
            prev_index,
            sequence,
        });
    }
    let n_out = r.read_varint()? as usize;
    if n_out > 256 {
        return Err(format!("unreasonable output count {n_out}"));
    }
    let mut outputs = Vec::with_capacity(n_out);
    for _ in 0..n_out {
        let start = r.pos();
        // value
        r.skip(8)?;
        // addr_version, addr_len, addr_hash
        let _ver = r.read_u8()?;
        let addr_len = r.read_u8()? as usize;
        if addr_len != 20 && addr_len != 32 {
            return Err(format!("unknown addr_len {addr_len}"));
        }
        r.skip(addr_len)?;
        // covenant kind
        r.skip(1)?;
        // covenant items
        let n_items = r.read_varint()? as usize;
        for _ in 0..n_items {
            let item_len = r.read_varint()? as usize;
            r.skip(item_len)?;
        }
        outputs.push(&buf[start..r.pos()]);
    }
    let locktime = r.read_u32_le()?;
    // fbd's unsigned form ends here; witnesses aren't present.
    Ok(ParsedTx {
        version,
        locktime,
        inputs,
        outputs,
    })
}

struct TxReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> TxReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn pos(&self) -> usize {
        self.pos
    }
    fn need(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).ok_or("tx reader overflow")?;
        if end > self.buf.len() {
            return Err(format!(
                "tx truncated at pos {}: need {n}, have {}",
                self.pos,
                self.buf.len() - self.pos
            ));
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn skip(&mut self, n: usize) -> Result<(), String> {
        self.need(n).map(|_| ())
    }
    fn read_u8(&mut self) -> Result<u8, String> {
        Ok(self.need(1)?[0])
    }
    fn read_u32_le(&mut self) -> Result<u32, String> {
        let b = self.need(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let s = self.need(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }
    fn read_varint(&mut self) -> Result<u64, String> {
        let first = self.read_u8()?;
        Ok(match first {
            0xff => {
                let b = self.need(8)?;
                u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            }
            0xfe => self.read_u32_le()? as u64,
            0xfd => {
                let b = self.need(2)?;
                u16::from_le_bytes([b[0], b[1]]) as u64
            }
            n => n as u64,
        })
    }
}

fn write_varint(buf: &mut Vec<u8>, n: u64) {
    if n < 0xfd {
        buf.push(n as u8);
    } else if n <= 0xffff {
        buf.push(0xfd);
        buf.extend_from_slice(&(n as u16).to_le_bytes());
    } else if n <= 0xffff_ffff {
        buf.push(0xfe);
        buf.extend_from_slice(&(n as u32).to_le_bytes());
    } else {
        buf.push(0xff);
        buf.extend_from_slice(&n.to_le_bytes());
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

// ─── Tauri command bodies ───────────────────────────────────────────────────

/// `transport` selects the wire: `"usb"`, `"ble"`, or `"auto"` (USB first,
/// falls back to BLE if a `device_id` is provided and no USB Ledger was
/// found). `device_id` is required for BLE and ignored for USB. On mobile
/// (no USB) `"usb"` errors and `"auto"` goes straight to BLE.
pub fn cmd_get_account_xpub(
    account: u32,
    transport: &str,
    device_id: Option<&str>,
) -> Result<String, String> {
    #[cfg(desktop)]
    {
        match transport {
            "usb" => xpub_hid(account),
            "ble" => xpub_ble(account, device_id.unwrap_or_default()),
            _ => match xpub_hid(account) {
                Ok(v) => Ok(v),
                Err(e) if is_no_usb_device(&e) => match device_id {
                    Some(id) if !id.is_empty() => xpub_ble(account, id),
                    _ => Err(e),
                },
                Err(e) => Err(e),
            },
        }
    }
    #[cfg(not(desktop))]
    {
        match transport {
            "usb" => Err("USB Ledger is not supported on this platform".into()),
            _ => xpub_ble(account, device_id.unwrap_or_default()),
        }
    }
}

/// Sign an arbitrary message with the Ledger key at the given BIP-32
/// path. Used by the wallet's `signmessagewithname` flow when the active
/// wallet is watch-only: fbd can't sign (no local key), so we compute the
/// path client-side (via `listaddresses`) and route through the device.
pub fn cmd_sign_message(
    path: &str,
    message: &str,
    transport: &str,
    device_id: Option<&str>,
) -> Result<String, String> {
    let path_words = parse_path(path)?;
    let msg_bytes = message.as_bytes().to_vec();
    #[cfg(desktop)]
    {
        match transport {
            "usb" => sign_message_hid(&path_words, &msg_bytes),
            "ble" => sign_message_ble(&path_words, &msg_bytes, device_id.unwrap_or_default()),
            _ => match sign_message_hid(&path_words, &msg_bytes) {
                Ok(v) => Ok(v),
                Err(e) if is_no_usb_device(&e) => match device_id {
                    Some(id) if !id.is_empty() => sign_message_ble(&path_words, &msg_bytes, id),
                    _ => Err(e),
                },
                Err(e) => Err(e),
            },
        }
    }
    #[cfg(not(desktop))]
    {
        match transport {
            "usb" => Err("USB Ledger is not supported on this platform".into()),
            _ => sign_message_ble(&path_words, &msg_bytes, device_id.unwrap_or_default()),
        }
    }
}

#[cfg(desktop)]
fn sign_message_hid(path: &[u32], message: &[u8]) -> Result<String, String> {
    let path = path.to_vec();
    let message = message.to_vec();
    hid::dispatch(move |api| -> Result<String, String> {
        let ledger = Ledger::new(HidTransport::open(api)?);
        let sig = ledger.sign_message(&path, &message)?;
        Ok(base64_encode(&sig))
    })
}

fn sign_message_ble(path: &[u32], message: &[u8], device_id: &str) -> Result<String, String> {
    if device_id.is_empty() {
        return Err("BLE transport requires a deviceId".into());
    }
    ble::connect(device_id)?;
    let ledger = Ledger::new(BleTransport::get_connected(device_id)?);
    let sig = ledger.sign_message(path, message)?;
    Ok(base64_encode(&sig))
}

/// Standard Base64 encoder (RFC 4648, no padding stripped). Used for
/// wire-compat with fbd's `signmessage`/`verifymessage` which also
/// emit/accept base64. Inline to avoid pulling in a whole crate.
fn base64_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let a = bytes[i];
        let b = bytes[i + 1];
        let c = bytes[i + 2];
        out.push(CHARS[(a >> 2) as usize] as char);
        out.push(CHARS[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        out.push(CHARS[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char);
        out.push(CHARS[(c & 0x3f) as usize] as char);
        i += 3;
    }
    match bytes.len() - i {
        1 => {
            let a = bytes[i];
            out.push(CHARS[(a >> 2) as usize] as char);
            out.push(CHARS[((a & 0x03) << 4) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let a = bytes[i];
            let b = bytes[i + 1];
            out.push(CHARS[(a >> 2) as usize] as char);
            out.push(CHARS[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
            out.push(CHARS[((b & 0x0f) << 2) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

pub fn cmd_sign_pstx(
    pstx_hex: &str,
    network: &str,
    address_to_path: &HashMap<String, String>,
    transport: &str,
    device_id: Option<&str>,
) -> Result<String, String> {
    #[cfg(desktop)]
    {
        match transport {
            "usb" => sign_pstx_hid(pstx_hex, network, address_to_path),
            "ble" => sign_pstx_ble(pstx_hex, network, address_to_path, device_id.unwrap_or_default()),
            _ => match sign_pstx_hid(pstx_hex, network, address_to_path) {
                Ok(v) => Ok(v),
                Err(e) if is_no_usb_device(&e) => match device_id {
                    Some(id) if !id.is_empty() => sign_pstx_ble(pstx_hex, network, address_to_path, id),
                    _ => Err(e),
                },
                Err(e) => Err(e),
            },
        }
    }
    #[cfg(not(desktop))]
    {
        match transport {
            "usb" => Err("USB Ledger is not supported on this platform".into()),
            _ => sign_pstx_ble(pstx_hex, network, address_to_path, device_id.unwrap_or_default()),
        }
    }
}

#[cfg(desktop)]
fn xpub_hid(account: u32) -> Result<String, String> {
    hid::dispatch(move |api| -> Result<String, String> {
        let ledger = Ledger::new(HidTransport::open(api)?);
        fetch_xpub(&ledger, account)
    })
}

fn xpub_ble(account: u32, device_id: &str) -> Result<String, String> {
    if device_id.is_empty() {
        return Err("BLE transport requires a deviceId".into());
    }
    ble::connect(device_id)?;
    let ledger = Ledger::new(BleTransport::get_connected(device_id)?);
    fetch_xpub(&ledger, account)
}

fn fetch_xpub<T: LedgerTransport>(ledger: &Ledger<T>, account: u32) -> Result<String, String> {
    let path = vec![HARDENED | 44, HARDENED | SLIP44_FBC, HARDENED | account];
    let (pubkey, chain_code, _) = ledger.get_pubkey(&path, false)?;
    assemble_xpub(account, &pubkey, &chain_code)
}

#[cfg(desktop)]
fn sign_pstx_hid(
    pstx_hex: &str,
    network: &str,
    address_to_path: &HashMap<String, String>,
) -> Result<String, String> {
    // Clone borrowed inputs so the closure can be `'static` for the dispatch thread.
    let pstx_hex = pstx_hex.to_string();
    let network = network.to_string();
    let address_to_path = address_to_path.clone();
    hid::dispatch(move |api| -> Result<String, String> {
        let pstx_bytes = hex::decode(pstx_hex.trim()).map_err(|e| format!("hex decode: {e}"))?;
        let pstx = PstxV1::parse(&pstx_bytes)?;
        let ledger = Ledger::new(HidTransport::open(api)?);
        sign_parsed_pstx(&pstx, &network, &address_to_path, &ledger)
    })
}

fn sign_pstx_ble(
    pstx_hex: &str,
    network: &str,
    address_to_path: &HashMap<String, String>,
    device_id: &str,
) -> Result<String, String> {
    if device_id.is_empty() {
        return Err("BLE transport requires a deviceId".into());
    }
    let pstx_bytes = hex::decode(pstx_hex.trim()).map_err(|e| format!("hex decode: {e}"))?;
    let pstx = PstxV1::parse(&pstx_bytes)?;
    ble::connect(device_id)?;
    let ledger = Ledger::new(BleTransport::get_connected(device_id)?);
    sign_parsed_pstx(&pstx, network, address_to_path, &ledger)
}

fn sign_parsed_pstx<T: LedgerTransport>(
    pstx: &PstxV1,
    network: &str,
    address_to_path: &HashMap<String, String>,
    ledger: &Ledger<T>,
) -> Result<String, String> {
    // Build one SignInput per pstx coin, then hand them all to the device
    // in a single signing session. One approval covers the whole tx.
    let mut inputs = Vec::with_capacity(pstx.coins.len());
    for (i, coin) in pstx.coins.iter().enumerate() {
        let addr = encode_bech32_address(network, coin.address_version, &coin.address_hash)?;
        let path_str = address_to_path
            .get(&addr)
            .ok_or_else(|| format!("no derivation path for input {i} address {addr}"))?;
        inputs.push(SignInput {
            path: parse_path(path_str)?,
            value: coin.value,
            sighash_type: 0x01,
        });
    }
    let results = ledger.sign_pstx(&pstx.tx_bytes, &inputs)?;
    let input_pairs: Vec<(Vec<u8>, Vec<u8>)> = results
        .into_iter()
        .map(|r| {
            let mut sig = r.sig;
            sig.push(r.sighash_byte);
            (sig, r.pubkey)
        })
        .collect();
    let signed = pstx.serialize_p2wpkh(&input_pairs)?;
    Ok(hex::encode(signed))
}

#[cfg(desktop)]
fn is_no_usb_device(err: &str) -> bool {
    err.starts_with("no Ledger device found")
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

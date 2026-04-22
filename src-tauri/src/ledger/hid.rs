//! USB HID transport for Ledger devices.
//!
//! Ledger devices expose a raw HID interface that speaks APDUs wrapped in
//! fixed-size 64-byte frames. This module:
//!
//! * Owns a single dedicated thread (`fistbump-ledger-hid`) that services
//!   all HID operations via `dispatch`. macOS IOKit
//!   (`IOHIDManagerSetDeviceMatchingMultiple`, `IOHIDDeviceScheduleWithRunLoop`)
//!   schedules HID notification sources on the *calling thread's* CFRunLoop.
//!   When two tokio worker threads ever touch the same `HidApi` (or
//!   `HidDevice`s derived from it), CFRunLoop's internal info gets mutated
//!   concurrently and the next call PAC-traps in
//!   `__CFCheckCFInfoPACSignature`. Serializing through a single thread —
//!   not just a mutex — gives IOKit a stable run loop to live on.
//! * Implements `LedgerTransport` via `HidTransport`, handling the APDU
//!   chunking into HID frames.

use std::sync::mpsc::{channel, sync_channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use hidapi::{HidApi, HidDevice};

use super::{LedgerTransport, SW_OK};

const LEDGER_VID: u16 = 0x2c97;
const HID_CID: u16 = 0x0101;
const HID_TAG_APDU: u8 = 0x05;
const HID_FRAME_LEN: usize = 64;

type Job = Box<dyn FnOnce(&mut HidApi) + Send + 'static>;

static JOB_TX: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();

/// Run `f` on the dedicated HID thread with access to the shared `HidApi`.
pub fn dispatch<F, R>(f: F) -> Result<R, String>
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

pub struct HidTransport {
    device: HidDevice,
}

impl HidTransport {
    /// Open the first Ledger device on USB HID.
    ///
    /// **Must run on the dedicated HID thread** — see `dispatch`. Refreshes
    /// the device list on each call so a newly-plugged Ledger is found.
    pub fn open(api: &mut HidApi) -> Result<Self, String> {
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
            let device = api
                .open_path(info.path())
                .map_err(|e| format!("open Ledger device failed: {e}"))?;
            return Ok(Self { device });
        }
        Err("no Ledger device found — connect & unlock the device, then open Fistbump".into())
    }
}

impl LedgerTransport for HidTransport {
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
}

//! Bluetooth Low Energy transport for Ledger devices (Nano X, Stax, Flex).
//!
//! Ledger's BLE APDU protocol mirrors its USB HID one minus the channel ID:
//! frame `[tag(1) seq(2 BE) payload]` in chunks of `mtu - 3`, first frame's
//! payload prefixed with the total APDU length (2 BE). Tag `0x05` is APDU,
//! `0x08` is MTU negotiation. MTU is negotiated once at connect time by
//! writing `[0x08, 0, 0, 0, 0]`; the device replies with
//! `[0x08, 0, 0, 0, 5, MTU, 0, 0, 0, 0]`.
//!
//! Connections are cached globally (keyed by the btleplug device-id string)
//! so repeat signing calls reuse an open GATT session instead of rescanning
//! and re-pairing.
//!
//! UUIDs are sourced from `@ledgerhq/devices` in ledger-live.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, ValueNotification,
    WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::stream::{Stream, StreamExt};
use serde::Serialize;
use tokio::runtime::Runtime;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout as tokio_timeout;
use uuid::{uuid, Uuid};

use super::{LedgerTransport, SW_OK};

const BLE_APDU_TAG: u8 = 0x05;
const BLE_MTU_TAG: u8 = 0x08;
const DEFAULT_SCAN_SECS: u64 = 5;

// ─── Device models (from @ledgerhq/devices) ────────────────────────────────

struct BleModel {
    name: &'static str,
    service: Uuid,
    notify: Uuid,
    write_cmd: Uuid,
}

const LEDGER_MODELS: &[BleModel] = &[
    BleModel {
        name: "Nano X",
        service:   uuid!("13d63400-2c97-0004-0000-4c6564676572"),
        notify:    uuid!("13d63400-2c97-0004-0001-4c6564676572"),
        write_cmd: uuid!("13d63400-2c97-0004-0003-4c6564676572"),
    },
    BleModel {
        name: "Stax",
        service:   uuid!("13d63400-2c97-6004-0000-4c6564676572"),
        notify:    uuid!("13d63400-2c97-6004-0001-4c6564676572"),
        write_cmd: uuid!("13d63400-2c97-6004-0003-4c6564676572"),
    },
    BleModel {
        name: "Flex",
        service:   uuid!("13d63400-2c97-3004-0000-4c6564676572"),
        notify:    uuid!("13d63400-2c97-3004-0001-4c6564676572"),
        write_cmd: uuid!("13d63400-2c97-3004-0003-4c6564676572"),
    },
];

fn model_for_service(service: &Uuid) -> Option<&'static BleModel> {
    LEDGER_MODELS.iter().find(|m| &m.service == service)
}

fn model_for_name(name: &str) -> Option<&'static BleModel> {
    LEDGER_MODELS.iter().find(|m| name.contains(m.name))
}

// ─── Runtime + connection cache ────────────────────────────────────────────

/// Dedicated runtime for BLE ops so we can `block_on` without touching
/// tauri's runtime.
///
/// Single-threaded on purpose: on Android, btleplug's droidplug requires
/// the thread running its Java callbacks to be attached to the JVM. A
/// multi-thread runtime would move futures between worker threads and
/// force us to attach each one; pinning everything to the block_on
/// caller's thread lets us attach once per op and call it a day.
fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build ledger BLE runtime")
    })
}

type NotificationStream = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

struct Conn {
    peripheral: Peripheral,
    write_cmd: Characteristic,
    notify_uuid: Uuid,
    mtu: usize,
    notifications: AsyncMutex<NotificationStream>,
}

fn connections() -> &'static Mutex<HashMap<String, Arc<Conn>>> {
    static CONNECTIONS: OnceLock<Mutex<HashMap<String, Arc<Conn>>>> = OnceLock::new();
    CONNECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

// ─── Public API ────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct BleDeviceInfo {
    pub id: String,
    pub name: String,
    pub model: String,
}

/// Scan for Ledger BLE devices for `timeout_secs` (or 5s if 0).
pub fn scan(timeout_secs: u32) -> Result<Vec<BleDeviceInfo>, String> {
    let duration = Duration::from_secs(if timeout_secs == 0 {
        DEFAULT_SCAN_SECS
    } else {
        timeout_secs as u64
    });
    rt().block_on(async move {
        eprintln!("[ledger-ble] scan: resolving adapter");
        let adapter = tokio_timeout(Duration::from_secs(10), get_adapter())
            .await
            .map_err(|_| "BLE adapter not ready (is Bluetooth enabled and Fistbump allowed to use it?)".to_string())??;
        eprintln!("[ledger-ble] scan: adapter ready, starting scan");
        // Not filtering by service UUID at the OS layer — Stax in particular
        // doesn't always advertise it. Post-filter the results ourselves.
        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|e| format!("BLE scan start: {e}"))?;
        eprintln!("[ledger-ble] scan: scanning for {}s", duration.as_secs());
        tokio::time::sleep(duration).await;
        let peripherals = adapter
            .peripherals()
            .await
            .map_err(|e| format!("BLE peripherals: {e}"))?;
        eprintln!("[ledger-ble] scan: {} peripherals seen", peripherals.len());
        let _ = adapter.stop_scan().await;

        let mut out = Vec::new();
        for p in peripherals {
            let Ok(Some(props)) = p.properties().await else { continue };
            eprintln!(
                "[ledger-ble] scan: candidate id={} name={:?} services={}",
                p.id(),
                props.local_name,
                props.services.len()
            );
            let model = props
                .services
                .iter()
                .find_map(model_for_service)
                .or_else(|| props.local_name.as_deref().and_then(model_for_name));
            let Some(model) = model else { continue };
            out.push(BleDeviceInfo {
                id: p.id().to_string(),
                name: props
                    .local_name
                    .unwrap_or_else(|| format!("Ledger {}", model.name)),
                model: model.name.to_string(),
            });
        }
        eprintln!("[ledger-ble] scan: returning {} Ledger device(s)", out.len());
        Ok(out)
    })
}

/// Pair (if needed) and cache the GATT session. Idempotent — but if a
/// cached session's peripheral is no longer connected (OS dropped the
/// link between ops, Android is especially good at this), we drop it
/// and reconnect from scratch.
pub fn connect(device_id: &str) -> Result<String, String> {
    let device_id = device_id.to_string();
    rt().block_on(async move {
        let cached = connections().lock().unwrap().get(&device_id).cloned();
        if let Some(conn) = cached {
            if conn.peripheral.is_connected().await.unwrap_or(false) {
                return Ok(device_id);
            }
            eprintln!("[ledger-ble] connect: cached session dropped, reconnecting");
            connections().lock().unwrap().remove(&device_id);
        }
        eprintln!("[ledger-ble] connect: resolving adapter");
        let adapter = get_adapter().await?;
        eprintln!("[ledger-ble] connect: finding peripheral {device_id}");
        let peripheral = find_peripheral(&adapter, &device_id).await?;

        if !peripheral.is_connected().await.unwrap_or(false) {
            eprintln!("[ledger-ble] connect: GATT connecting");
            // GATT connect can wait on OS-level pairing on Ledger Stax; give
            // it a long ceiling rather than hanging forever.
            tokio_timeout(Duration::from_secs(60), peripheral.connect())
                .await
                .map_err(|_| "BLE connect timed out — re-try after pairing finishes".to_string())?
                .map_err(|e| format!("BLE connect: {e}"))?;
        }
        eprintln!("[ledger-ble] connect: discovering services");
        tokio_timeout(Duration::from_secs(30), peripheral.discover_services())
            .await
            .map_err(|_| "BLE service discovery timed out".to_string())?
            .map_err(|e| format!("BLE discover services: {e}"))?;

        let chars = peripheral.characteristics();
        eprintln!("[ledger-ble] connect: {} characteristics discovered", chars.len());
        let model = LEDGER_MODELS
            .iter()
            .find(|m| chars.iter().any(|c| c.service_uuid == m.service))
            .ok_or_else(|| "device doesn't expose a known Ledger BLE service".to_string())?;
        eprintln!("[ledger-ble] connect: matched model {}", model.name);
        let notify_char = chars
            .iter()
            .find(|c| c.uuid == model.notify)
            .cloned()
            .ok_or_else(|| format!("{} notify characteristic missing", model.name))?;
        let write_cmd = chars
            .iter()
            .find(|c| c.uuid == model.write_cmd)
            .cloned()
            .ok_or_else(|| format!("{} write-cmd characteristic missing", model.name))?;

        // Get the notifications stream *before* subscribing — btleplug's
        // stream is a broadcast; calling subscribe first can race the
        // first response (the MTU reply) past a not-yet-attached listener.
        eprintln!("[ledger-ble] connect: opening notifications stream");
        let mut notifications: NotificationStream = peripheral
            .notifications()
            .await
            .map_err(|e| format!("BLE notifications: {e}"))?;
        eprintln!("[ledger-ble] connect: subscribing to notify char");
        tokio_timeout(Duration::from_secs(30), peripheral.subscribe(&notify_char))
            .await
            .map_err(|_| "BLE subscribe timed out (CCCD write may require pairing)".to_string())?
            .map_err(|e| format!("BLE subscribe: {e}"))?;

        eprintln!("[ledger-ble] connect: negotiating MTU");
        let mtu = negotiate_mtu(&peripheral, &write_cmd, &mut notifications, model.notify).await?;
        eprintln!("[ledger-ble] connect: MTU={mtu}, ready");

        let conn = Arc::new(Conn {
            peripheral,
            write_cmd,
            notify_uuid: model.notify,
            mtu,
            notifications: AsyncMutex::new(notifications),
        });
        connections()
            .lock()
            .unwrap()
            .insert(device_id.clone(), conn);
        Ok(device_id)
    })
}

/// Drop the cached session and disconnect the peripheral.
pub fn forget(device_id: &str) -> Result<(), String> {
    let conn = connections().lock().unwrap().remove(device_id);
    if let Some(conn) = conn {
        rt().block_on(async move {
            let _ = conn.peripheral.disconnect().await;
        });
    }
    Ok(())
}

/// Look up a peripheral by id in the adapter's cache; if not seen, run a
/// short scan to populate it. Needed because on a fresh adapter (or after
/// an app restart) the peripheral list is empty until we scan.
async fn find_peripheral(adapter: &Adapter, device_id: &str) -> Result<Peripheral, String> {
    if let Ok(peripherals) = adapter.peripherals().await {
        for p in peripherals {
            if p.id().to_string() == device_id {
                return Ok(p);
            }
        }
    }
    let _ = adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|e| format!("BLE scan: {e}"))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(400)).await;
        if let Ok(peripherals) = adapter.peripherals().await {
            for p in peripherals {
                if p.id().to_string() == device_id {
                    let _ = adapter.stop_scan().await;
                    return Ok(p);
                }
            }
        }
    }
    let _ = adapter.stop_scan().await;
    Err("BLE device not reachable — make sure it's powered on and in range".into())
}

async fn get_adapter() -> Result<Adapter, String> {
    #[cfg(target_os = "android")]
    {
        android::init_btleplug()?;
        // btleplug's droidplug calls into its Java companion from the
        // current thread without attaching first — it expects us to have
        // done so. Tokio-spawned threads start detached, so attach now.
        crate::android_ble::attach_current_thread()?;
    }

    let manager = Manager::new()
        .await
        .map_err(|e| format!("BLE manager: {e}"))?;
    let adapters = manager
        .adapters()
        .await
        .map_err(|e| format!("BLE adapters: {e}"))?;
    adapters
        .into_iter()
        .next()
        .ok_or_else(|| "no BLE adapter found — is Bluetooth enabled?".to_string())
}

#[cfg(target_os = "android")]
mod android {
    //! Android permission gate. The actual btleplug droidplug init and
    //! JNI-to-Kotlin plumbing lives in `lib.rs::android_ble` (it's called
    //! directly from Kotlin so it has to sit at the crate root). Here we
    //! just memoize the permission prompt so it only fires on the first
    //! BLE op per process.

    use std::sync::OnceLock;

    static PERMISSIONS_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

    pub fn init_btleplug() -> Result<(), String> {
        PERMISSIONS_RESULT
            .get_or_init(crate::android_ble::ensure_permissions)
            .clone()
    }
}

async fn negotiate_mtu(
    peripheral: &Peripheral,
    write_cmd: &Characteristic,
    notifications: &mut NotificationStream,
    notify_uuid: Uuid,
) -> Result<usize, String> {
    let handshake = [BLE_MTU_TAG, 0x00, 0x00, 0x00, 0x00];
    peripheral
        .write(write_cmd, &handshake, WriteType::WithoutResponse)
        .await
        .map_err(|e| format!("BLE MTU write: {e}"))?;
    loop {
        let notif = tokio_timeout(Duration::from_secs(5), notifications.next())
            .await
            .map_err(|_| "BLE MTU negotiation timeout".to_string())?
            .ok_or_else(|| "BLE notification stream ended".to_string())?;
        if notif.uuid != notify_uuid {
            continue;
        }
        if notif.value.len() >= 6 && notif.value[0] == BLE_MTU_TAG {
            return Ok(notif.value[5] as usize);
        }
    }
}

// ─── BleTransport ──────────────────────────────────────────────────────────

pub struct BleTransport {
    conn: Arc<Conn>,
}

impl BleTransport {
    /// Grab a handle to an already-connected device.
    pub fn get_connected(device_id: &str) -> Result<Self, String> {
        let conn = connections()
            .lock()
            .unwrap()
            .get(device_id)
            .cloned()
            .ok_or_else(|| "device not connected — run ble_scan + ble_connect first".to_string())?;
        Ok(Self { conn })
    }
}

impl LedgerTransport for BleTransport {
    fn exchange(&self, apdu: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
        let conn = self.conn.clone();
        let apdu = apdu.to_vec();
        rt().block_on(async move { exchange_async(&conn, &apdu, timeout).await })
    }
}

async fn exchange_async(
    conn: &Conn,
    apdu: &[u8],
    timeout_dur: Duration,
) -> Result<Vec<u8>, String> {
    let mut notifications = conn.notifications.lock().await;

    // Outbound: chunk (length-prefixed) APDU into BLE frames.
    let total_len = u16::try_from(apdu.len()).map_err(|_| "apdu too long")?;
    let mut payload = Vec::with_capacity(2 + apdu.len());
    payload.extend_from_slice(&total_len.to_be_bytes());
    payload.extend_from_slice(apdu);

    let chunk_cap = conn.mtu.checked_sub(3).ok_or("BLE MTU too small")?;
    eprintln!(
        "[ledger-ble] exchange: sending APDU cla={:02x} ins={:02x} p1={:02x} p2={:02x} ({} bytes), mtu={} chunk_cap={}",
        apdu.first().copied().unwrap_or(0),
        apdu.get(1).copied().unwrap_or(0),
        apdu.get(2).copied().unwrap_or(0),
        apdu.get(3).copied().unwrap_or(0),
        apdu.len(),
        conn.mtu,
        chunk_cap,
    );
    let mut seq: u16 = 0;
    let mut offset = 0;
    while offset < payload.len() {
        let chunk_size = (payload.len() - offset).min(chunk_cap);
        let mut frame = Vec::with_capacity(3 + chunk_size);
        frame.push(BLE_APDU_TAG);
        frame.extend_from_slice(&seq.to_be_bytes());
        frame.extend_from_slice(&payload[offset..offset + chunk_size]);
        conn.peripheral
            .write(&conn.write_cmd, &frame, WriteType::WithoutResponse)
            .await
            .map_err(|e| format!("BLE write: {e}"))?;
        eprintln!("[ledger-ble] exchange: wrote frame seq={seq} size={chunk_size}");
        offset += chunk_size;
        seq = seq.checked_add(1).ok_or("BLE seq overflow")?;
    }
    eprintln!("[ledger-ble] exchange: awaiting response (timeout {}s)", timeout_dur.as_secs());

    // Inbound: re-assemble response frames.
    let mut response = Vec::new();
    let mut expected: Option<usize> = None;
    let mut seq: u16 = 0;
    loop {
        let notif = tokio_timeout(timeout_dur, notifications.next())
            .await
            .map_err(|_| "BLE read timeout".to_string())?
            .ok_or_else(|| "BLE notification stream ended".to_string())?;
        if notif.uuid != conn.notify_uuid {
            eprintln!(
                "[ledger-ble] exchange: ignoring notification from uuid {} (want {})",
                notif.uuid, conn.notify_uuid
            );
            continue;
        }
        let data = notif.value;
        eprintln!("[ledger-ble] exchange: got {} bytes on notify", data.len());
        if data.len() < 3 || data[0] != BLE_APDU_TAG {
            return Err(format!(
                "bad BLE frame: tag={:02x} len={}",
                data.first().copied().unwrap_or(0),
                data.len()
            ));
        }
        let got_seq = u16::from_be_bytes([data[1], data[2]]);
        if got_seq != seq {
            return Err(format!("BLE seq mismatch: expected {seq}, got {got_seq}"));
        }
        let mut frame_payload = &data[3..];
        if seq == 0 {
            if frame_payload.len() < 2 {
                return Err("first BLE frame missing total length".into());
            }
            expected = Some(u16::from_be_bytes([frame_payload[0], frame_payload[1]]) as usize);
            frame_payload = &frame_payload[2..];
        }
        response.extend_from_slice(frame_payload);
        seq = seq.checked_add(1).ok_or("BLE seq overflow on read")?;
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

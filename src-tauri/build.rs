fn main() {
    tauri_build::build();

    let conf = std::fs::read_to_string("fistbump.conf.json").expect("failed to read fistbump.conf.json");
    let val: serde_json::Value = serde_json::from_str(&conf).expect("failed to parse fistbump.conf.json");
    let network = val["network"].as_str().unwrap_or("testnet");
    println!("cargo:rustc-env=FISTBUMP_NETWORK={}", network);
    println!("cargo:rerun-if-changed=fistbump.conf.json");

    // Inject the wallet repo's git commit hash at compile time. We tell
    // Cargo to re-run when HEAD or any common branch ref changes so the
    // embedded hash never goes stale because of the incremental cache.
    let wallet_hash = std::process::Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=WALLET_BUILD_HASH={}", wallet_hash);
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    for r in &["primary", "main", "master"] {
        println!("cargo:rerun-if-changed=.git/refs/heads/{}", r);
        println!("cargo:rerun-if-changed=../.git/refs/heads/{}", r);
    }

    // Inject the BUNDLED fbd's git commit hash. build-all.sh runs
    // generate-build-info.sh before each fbd build, which writes the
    // current fbd commit hash into Sources/Base/BuildInfo.swift. We
    // read that file here so the wallet binary embeds the hash of
    // whatever fbd it is about to bundle. Without this, Cargo's
    // incremental cache silently keeps a stale hash even after fbd is
    // rebuilt and users couldn't tell from the About dialog what fbd
    // version is actually inside.
    let fbd_buildinfo = "../../fbd/Sources/Base/BuildInfo.swift";
    let fbd_hash = std::fs::read_to_string(fbd_buildinfo)
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.contains("_buildHash"))
                .and_then(|l| l.split('"').nth(1))
                .map(String::from)
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=FBD_BUNDLED_HASH={}", fbd_hash);
    println!("cargo:rerun-if-changed={}", fbd_buildinfo);

    // Xcode 26 removed Swift back-deployment compatibility libraries that
    // Tauri's swift-rs bridge still references. Compile stub symbols so
    // the linker doesn't fail on iOS builds.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "ios" {
        cc::Build::new()
            .file("swift-compat-stubs.c")
            .compile("swift_compat_stubs");
        // btleplug's CoreBluetooth backend (Ledger Nano X / Stax / Flex pair
        // over BLE) references CoreBluetooth symbols such as
        // CBAdvertisementDataManufacturerDataKey. A Rust staticlib doesn't carry
        // its framework deps, so the iOS link fails with "symbol(s) not found"
        // unless we link CoreBluetooth explicitly here.
        println!("cargo:rustc-link-lib=framework=CoreBluetooth");
    }
}

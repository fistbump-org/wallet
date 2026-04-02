fn main() {
    tauri_build::build();

    let conf = std::fs::read_to_string("fistbump.conf.json").expect("failed to read fistbump.conf.json");
    let val: serde_json::Value = serde_json::from_str(&conf).expect("failed to parse fistbump.conf.json");
    let network = val["network"].as_str().unwrap_or("testnet");
    println!("cargo:rustc-env=FISTBUMP_NETWORK={}", network);
    println!("cargo:rerun-if-changed=fistbump.conf.json");

    // Inject git commit hash at compile time
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    println!("cargo:rustc-env=WALLET_BUILD_HASH={}", hash.trim());

    // Xcode 26 removed Swift back-deployment compatibility libraries that
    // Tauri's swift-rs bridge still references. Compile stub symbols so
    // the linker doesn't fail on iOS builds.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "ios" {
        cc::Build::new()
            .file("swift-compat-stubs.c")
            .compile("swift_compat_stubs");
    }
}

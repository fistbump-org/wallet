// Stub symbols for Swift back-deployment libraries removed in Xcode 26.
// Tauri's Swift bridge (swift-rs) references these when compiled with
// swift-tools-version:5.3 targeting iOS 11+. They are harmless no-ops
// since we require iOS 16+ where the runtime has these built in.
void _swift_FORCE_LOAD_$_swiftCompatibility56(void) {}
void _swift_FORCE_LOAD_$_swiftCompatibilityConcurrency(void) {}
void _swift_FORCE_LOAD_$_swiftCompatibilityPacks(void) {}

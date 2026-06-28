#include "bindings/bindings.h"

// Start the in-process fbd node before handing control to Tauri. startFBDNode()
// (FBDNode.swift, exported as @_cdecl "start_fbd_node") registers the FFI
// handlers, bootstraps swift-log, starts the DANE proxy, and launches the node
// on a background task. It must run BEFORE ffi::start_app(), which enters the
// UIApplication run loop and does not return.
//
// This file is an overlay: sync-mobile-overlay.mjs symlinks it over the
// project-generated main.mm, so the node-start call survives `tauri ios init`
// regenerating gen/apple.
extern "C" void start_fbd_node(void);

int main(int argc, char * argv[]) {
	start_fbd_node();
	ffi::start_app();
	return 0;
}

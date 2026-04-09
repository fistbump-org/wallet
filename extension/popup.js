// Popup renders the extension version and a live "wallet running" status
// dot. The status check goes through the background service worker, which
// issues a native-messaging probe via `sendNative({type:'info'})`. The
// bridge has a short-circuit for info frames that never auto-launches the
// wallet — so clicking the toolbar icon never causes the desktop app to
// boot, which would be a surprising side-effect of a status check.
(function() {
  var versionEl = document.getElementById('version');
  var statusEl = document.getElementById('status');
  var labelEl = document.getElementById('status-label');

  // Fill the version right away from the manifest, then begin polling.
  try {
    var manifest = chrome.runtime.getManifest();
    if (manifest && manifest.version) {
      versionEl.textContent = 'v' + manifest.version;
    }
  } catch (_) {}

  function setStatus(state, label) {
    statusEl.classList.remove('ok', 'off');
    statusEl.classList.add(state);
    labelEl.textContent = label;
  }

  async function probe() {
    try {
      var res = await chrome.runtime.sendMessage({ type: 'info' });
      if (res && res.running) {
        setStatus('ok', 'Connected');
      } else {
        setStatus('off', 'Not running');
      }
    } catch (e) {
      setStatus('off', 'Not running');
    }
  }

  // Run once on open, then poll every 2 s while the popup is visible.
  // Chrome closes popups aggressively (any click outside or focus change
  // tears them down), so this interval is effectively bounded by how long
  // the user keeps the popup open.
  probe();
  var interval = setInterval(probe, 2000);
  window.addEventListener('unload', function() { clearInterval(interval); });
})();

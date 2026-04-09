// Runs in the extension's isolated world on every page. Two jobs:
//   1. Inject injected.js into the page's main world so the dApp can see
//      `window.fistbump`.
//   2. Relay postMessage traffic between the page (injected.js) and the
//      extension's background service worker, which actually talks to the
//      wallet's local HTTP API.
(function() {
  var s = document.createElement('script');
  s.src = chrome.runtime.getURL('injected.js');
  s.onload = function() { this.remove(); };
  (document.head || document.documentElement).appendChild(s);

  window.addEventListener('message', async function(event) {
    // Only messages from this window, targeted at us.
    if (event.source !== window) return;
    var msg = event.data;
    if (!msg || msg.target !== 'fistbump-cs' || !msg.id || !msg.type) return;

    var reply = { target: 'fistbump-page', id: msg.id };
    try {
      var response = await chrome.runtime.sendMessage({
        type: msg.type,
        payload: msg.payload || {},
        // Use the current page's origin, not the iframe's. We explicitly
        // limit content_scripts to top-level frames in manifest.json.
        origin: window.location.origin
      });
      if (response && response.error) {
        reply.error = response.error;
      } else {
        reply.response = response;
      }
    } catch (e) {
      reply.error = (e && e.message) ? e.message : String(e);
    }
    window.postMessage(reply, window.location.origin);
  });
})();

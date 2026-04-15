// MV3 service worker. Receives requests from the page-injected provider
// (relayed by content-script.js) and forwards them to the desktop wallet
// over Chrome's native messaging API.
//
// Native messaging spawns the bundled `fistbump-bridge` binary that lives
// inside Fistbump.app/Contents/Resources, which itself stdio-forwards
// length-prefixed JSON frames to the wallet's Unix socket at
// ~/.fistbump/extension.sock. The wallet does the actual work (popping
// the approval modal, looking up the address, etc.) and sends a response
// back the same way.
//
// Why native messaging end-to-end:
//   - Chrome verifies our extension's stable ID (locked via `key` in
//     manifest.json) against the host JSON's `allowed_origins` before
//     spawning the bridge, so other extensions can't impersonate us.
//   - The wallet has no HTTP listener for the extension to reach — there
//     is no localhost surface for a malicious page or other process to hit.
//   - If the wallet isn't running, the bridge launches it via `open` and
//     waits for the socket to come up before forwarding traffic.

var NM_HOST = 'org.fistbump.wallet';

chrome.runtime.onMessage.addListener(function(msg, sender, sendResponse) {
  handle(msg, sender)
    .then(function(result) { sendResponse(result); })
    .catch(function(err) { sendResponse({ error: (err && err.message) || String(err) }); });
  // Return true so Chrome keeps the sendResponse channel open for our async work.
  return true;
});

async function handle(msg, sender) {
  var origin = msg.origin;
  if (!origin && sender && sender.url) {
    try { origin = new URL(sender.url).origin; } catch (_) {}
  }

  if (msg.type === 'info') {
    return await sendNative({ type: 'info', origin: origin || '' });
  }
  if (msg.type === 'connect') {
    return await sendNative({ type: 'connect', origin: origin || '' });
  }
  if (msg.type === 'sendTx') {
    var payload = msg.payload || {};
    return await sendNative({
      type: 'sendTx',
      origin: origin || '',
      to: payload.to,
      amount: payload.amount,
    });
  }
  if (msg.type === 'signMessage') {
    var p = msg.payload || {};
    var sigReq = {
      type: 'signMessage',
      origin: origin || '',
      message: p.message,
    };
    if (p.name) sigReq.name = p.name;
    return await sendNative(sigReq);
  }
  if (msg.type === 'getPublicKey') {
    return await sendNative({ type: 'getPublicKey', origin: origin || '' });
  }
  if (msg.type === 'fundHtlc') {
    var fp = msg.payload || {};
    return await sendNative({
      type: 'fundHtlc',
      origin: origin || '',
      witnessScriptHex: fp.witnessScriptHex,
      amount: fp.amount,
      memo: fp.memo || '',
    });
  }
  if (msg.type === 'signHtlcSpend') {
    var sp = msg.payload || {};
    return await sendNative({
      type: 'signHtlcSpend',
      origin: origin || '',
      fundingTxid: sp.fundingTxid,
      fundingVout: sp.fundingVout,
      fundingAmount: sp.fundingAmount,
      witnessScriptHex: sp.witnessScriptHex,
      branch: sp.branch,
      preimageHex: sp.preimageHex || '',
      destinationAddress: sp.destinationAddress,
      feeRate: sp.feeRate,
    });
  }
  throw new Error('unknown request type: ' + msg.type);
}

// Send a single native messaging request and resolve with the parsed reply.
// Each call is its own short-lived port: we connect, post one message, wait
// for the response (or for the host to disconnect with an error), and tear
// down. The bridge launches the wallet on first connect when needed, so we
// don't need any retry/launch logic at this layer.
function sendNative(message) {
  return new Promise(function(resolve, reject) {
    var port;
    try {
      port = chrome.runtime.connectNative(NM_HOST);
    } catch (e) {
      reject(new Error('native messaging unavailable: ' + (e.message || e)));
      return;
    }

    var settled = false;
    function done(value, err) {
      if (settled) return;
      settled = true;
      try { port.disconnect(); } catch (_) {}
      if (err) reject(err);
      else resolve(value);
    }

    port.onMessage.addListener(function(reply) {
      done(reply, null);
    });
    port.onDisconnect.addListener(function() {
      var err = chrome.runtime.lastError && chrome.runtime.lastError.message;
      if (err) {
        if (/not found/i.test(err)) {
          done(null, new Error(
            'Fistbump native messaging host not installed. ' +
            'Open the Fistbump desktop app at least once to register it.'
          ));
        } else {
          done(null, new Error(err));
        }
      } else {
        // Disconnected without ever sending a reply.
        done(null, new Error('bridge closed without responding'));
      }
    });

    try {
      port.postMessage(message);
    } catch (e) {
      done(null, new Error('postMessage failed: ' + (e.message || e)));
    }
  });
}

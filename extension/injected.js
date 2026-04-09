// Runs in the page's main world. Defines `window.fistbump` and forwards
// requests to the content script (and from there to the extension service
// worker and the desktop wallet's local HTTP API) via window.postMessage.
//
// This file is loaded by content-script.js via a <script> tag so it runs
// in the page's JS context, not the extension's isolated world.
(function() {
  if (window.fistbump) return; // don't double-inject on iframes or reinjection

  var pending = new Map();
  var idCounter = 0;

  function request(type, payload) {
    return new Promise(function(resolve, reject) {
      var id = 'fb-' + (++idCounter) + '-' + Math.random().toString(36).slice(2, 8);
      pending.set(id, { resolve: resolve, reject: reject });
      window.postMessage(
        {
          target: 'fistbump-cs',
          id: id,
          type: type,
          payload: payload || {}
        },
        window.location.origin
      );
    });
  }

  window.addEventListener('message', function(event) {
    if (event.source !== window) return;
    var msg = event.data;
    if (!msg || msg.target !== 'fistbump-page' || !msg.id) return;
    var p = pending.get(msg.id);
    if (!p) return;
    pending.delete(msg.id);
    if (msg.error) {
      p.reject(new Error(msg.error));
    } else {
      p.resolve(msg.response);
    }
  });

  var listeners = {};

  // Unwrap a response from the background worker, turning wallet-side
  // `{error: "..."}` payloads into rejected promises so dApps can use
  // normal try/catch around await.
  function unwrap(promise) {
    return promise.then(function(res) {
      if (res && res.error) throw new Error(res.error);
      return res;
    });
  }

  var fistbump = {
    // Feature-detection flag so dApps can `if (window.fistbump?.isFistbump)`.
    isFistbump: true,

    // Ask the desktop wallet to approve a connection and return the address.
    // The user sees a confirmation modal inside the wallet UI on first connect;
    // subsequent calls from the same origin short-circuit without a prompt.
    connect: function() {
      return unwrap(request('connect'));
    },

    // Quick, non-prompting liveness check.
    isConnected: function() {
      return request('info').then(function(res) {
        return !!(res && res.connected);
      });
    },

    // Request the wallet to build, sign, and broadcast a transaction.
    // The wallet shows a confirm modal with the fee + total before signing,
    // and only signs after the user has unlocked (if the wallet is locked).
    // Resolves with `{ txid }`; rejects with an Error on denial or failure.
    sendTx: function(params) {
      if (!params || typeof params !== 'object') {
        return Promise.reject(new Error('sendTx: expected { to, amount }'));
      }
      if (typeof params.to !== 'string' || !params.to) {
        return Promise.reject(new Error('sendTx: missing `to` address'));
      }
      var amount = Number(params.amount);
      if (!(amount > 0 && isFinite(amount))) {
        return Promise.reject(new Error('sendTx: `amount` must be a positive number (FBC)'));
      }
      return unwrap(request('sendTx', { to: params.to, amount: amount }));
    },

    // Ask the wallet to sign a plain-text message (off-chain, no fee).
    // Two call shapes:
    //
    //   fistbump.signMessage('hello')
    //     → signs with the active wallet's receive address
    //     → resolves with { signature, address }
    //
    //   fistbump.signMessage('hello', { name: 'eskimo' })
    //   fistbump.signMessage({ message: 'hello', name: 'eskimo' })
    //     → signs with the key that owns the Fistbump name 'eskimo'
    //       (fbd's signmessagewithname RPC)
    //     → resolves with { signature, name }
    //
    // The wallet shows the message in a confirm modal first and displays
    // the signing identity (address or name) in the review.
    signMessage: function(input, opts) {
      var message, name;
      if (typeof input === 'string') {
        message = input;
        if (opts && typeof opts === 'object' && typeof opts.name === 'string') {
          name = opts.name;
        }
      } else if (input && typeof input === 'object') {
        message = input.message;
        if (typeof input.name === 'string') name = input.name;
      }
      if (typeof message !== 'string' || message.length === 0) {
        return Promise.reject(new Error('signMessage: expected a non-empty message'));
      }
      var payload = { message: message };
      if (name) payload.name = name;
      return unwrap(request('signMessage', payload));
    },

    // Placeholder event API — implemented fully in a later pass once
    // we actually have events to emit (account change, disconnect, etc.).
    on: function(event, fn) {
      if (!listeners[event]) listeners[event] = [];
      listeners[event].push(fn);
    },
    off: function(event, fn) {
      var arr = listeners[event];
      if (!arr) return;
      var i = arr.indexOf(fn);
      if (i >= 0) arr.splice(i, 1);
    }
  };

  Object.defineProperty(window, 'fistbump', {
    value: fistbump,
    writable: false,
    configurable: false
  });

  // dApps can listen for this to know the provider is ready.
  window.dispatchEvent(new Event('fistbump#initialized'));
})();

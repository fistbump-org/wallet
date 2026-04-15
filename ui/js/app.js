(function() {
  // Wait for layout to settle (safe area insets) then reveal
  setTimeout(function() {
    document.body.style.transition = 'opacity 0.15s';
    document.body.style.opacity = '1';
  }, 100);

  // Tauri API setup
  var __invoke = window.__TAURI__.core.invoke;
  var __currentWindow = window.__TAURI__.window.getCurrentWindow();

  var ua = navigator.userAgent || '';
  var plat = navigator.platform || '';
  var isIOS = /iPhone|iPad|iPod/.test(plat) || (/Mac/.test(plat) && 'ontouchend' in document);
  var isAndroid = /Android/i.test(ua);
  window.fistbump = {
    platform: isIOS ? 'ios' :
              isAndroid ? 'android' :
              plat.indexOf('Win') >= 0 ? 'win32' :
              plat.indexOf('Mac') >= 0 ? 'darwin' : 'linux',
    mobile: isIOS || isAndroid || window.innerWidth <= 768,
    minimize: function() { __currentWindow.minimize(); },
    maximize: function() { __currentWindow.toggleMaximize(); },
    close: function() { __currentWindow.close(); },
  };
  document.body.dataset.platform = window.fistbump.platform;

  // Mobile sidebar toggle
  var hamburgerBtn = document.getElementById('hamburger-btn');
  var sidebar = document.querySelector('.sidebar');
  var sidebarBackdrop = document.getElementById('sidebar-backdrop');

  function openSidebar() {
    sidebar.classList.add('open');
    sidebarBackdrop.classList.add('open');
  }
  function closeSidebar() {
    sidebar.classList.remove('open');
    sidebarBackdrop.classList.remove('open');
  }
  hamburgerBtn.addEventListener('click', function() {
    if (sidebar.classList.contains('open')) closeSidebar();
    else openSidebar();
  });
  sidebarBackdrop.addEventListener('click', closeSidebar);

  // Browse: phones use the mobile tab bar, tablets use the sidebar nav item.
  // Re-evaluate on resize so rotating an Android/iPad tablet between portrait
  // (< 768 = mobile tab bar) and landscape (≥ 768 = sidebar) swaps correctly.
  function updateBrowseNavVisibility() {
    var isMobilePlatform = window.fistbump.platform === 'ios' || window.fistbump.platform === 'android';
    var showInSidebar = isMobilePlatform && window.innerWidth > 768;
    document.getElementById('nav-browse').classList.toggle('hidden', !showInSidebar);
  }
  updateBrowseNavVisibility();
  window.addEventListener('resize', updateBrowseNavVisibility);

  // Open the native modal browser. Clicking Browse anywhere (mobile tab bar,
  // sidebar nav, or a programmatic call) routes through this — no inline
  // panel, no overlay positioning, the native screen owns everything.
  function openBrowser(url) {
    var isDark = !document.body.classList.contains('light');
    try {
      __invoke('browse', { url: url || '', dark: isDark });
    } catch(e) {
      if (url) __invoke('open_external', { url: url });
    }
  }

  // Mobile tab bar: Browse tab fires the modal without changing the active tab.
  if (window.fistbump.mobile) {
    var mobileTabs = document.querySelectorAll('.mobile-tab[data-mobile-tab]');
    mobileTabs.forEach(function(tab) {
      tab.addEventListener('click', function() {
        var target = tab.dataset.mobileTab;
        if (target === 'browser') {
          openBrowser();
          return;
        }
        mobileTabs.forEach(function(t) { t.classList.remove('active'); });
        tab.classList.add('active');
        if (target === 'settings') {
          document.querySelectorAll('.nav-item').forEach(function(b) { b.classList.remove('active'); });
          document.querySelectorAll('.page').forEach(function(p) { p.classList.remove('active'); });
          document.getElementById('tab-settings').classList.add('active');
        } else {
          document.querySelectorAll('.nav-item').forEach(function(b) { b.classList.remove('active'); });
          document.querySelectorAll('.page').forEach(function(p) { p.classList.remove('active'); });
          document.querySelector('.nav-item[data-tab="overview"]').classList.add('active');
          document.getElementById('tab-overview').classList.add('active');
        }
      });
    });
  }

  // Set app version + wallet build hash, and pre-populate the bundled
  // fbd hash. Both are baked into the wallet binary at compile time
  // (build.rs reads the wallet's git HEAD and fbd's BuildInfo.swift).
  // The "Node" row is also updated live by getblockchaininfo once fbd
  // connects, but the bundled hash gives an immediate answer at startup.
  try {
    Promise.all([
      window.__TAURI__.app.getVersion(),
      window.__TAURI__.core.invoke('get_wallet_build_hash'),
      window.__TAURI__.core.invoke('get_fbd_bundled_hash')
    ]).then(function(r) {
      var v = r[0], walletHash = r[1], fbdHash = r[2];
      var label = 'Fistbump ' + v;
      if (walletHash) label += ' (' + walletHash + ')';
      var appEl = document.getElementById('about-app');
      if (appEl) appEl.textContent = label;
      if (fbdHash && fbdHash !== 'unknown') {
        var nodeEl = document.getElementById('about-node');
        if (nodeEl && nodeEl.textContent === '--') {
          nodeEl.textContent = 'fbd ' + v + ' (' + fbdHash + ')';
        }
      }
    });
  } catch(e) {}

  // Window control buttons (must use addEventListener, not inline onclick — CSP blocks inline scripts in production)
  document.querySelectorAll('.tb-minimize').forEach(function(el) {
    el.addEventListener('click', function() { fistbump.minimize(); });
  });
  document.querySelectorAll('.tb-maximize').forEach(function(el) {
    el.addEventListener('click', function() { fistbump.maximize(); });
  });
  document.querySelectorAll('.tb-close').forEach(function(el) {
    el.addEventListener('click', function() { fistbump.close(); });
  });

  // Titlebar drag — stop propagation from interactive areas so startDragging
  // never fires when clicking buttons/search.
  document.querySelectorAll('.titlebar-controls, .titlebar-search').forEach(function(el) {
    el.addEventListener('mousedown', function(e) { e.stopPropagation(); });
  });
  document.querySelectorAll('.titlebar, .login-titlebar').forEach(function(el) {
    el.addEventListener('mousedown', function(e) {
      if (e.button !== 0) return;
      __currentWindow.startDragging();
    });
  });

  // Cmd+M to minimize on Mac
  document.addEventListener('keydown', function(e) {
    if ((e.metaKey || e.ctrlKey) && e.key === 'm') {
      e.preventDefault();
      fistbump.minimize();
    }
  });

  // Open external links in system browser with confirmation
  document.addEventListener('click', function(e) {
    var a = e.target.closest('a[target="_blank"]');
    if (a && a.href) {
      e.preventDefault();
      var url = a.href;
      showConfirm(url, { title: 'Open external link?', okText: 'Open' }).then(function(ok) {
        if (ok) __invoke('open_external', { url: url });
      });
    }
  });

  // Custom confirm/alert dialogs (native ones don't work on iOS WKWebView)
  function showConfirm(msg, opts) {
    return new Promise(function(resolve) {
      var overlay = document.getElementById('confirm-modal');
      var titleEl = document.getElementById('confirm-title');
      var title = opts && opts.title;
      if (title) {
        titleEl.textContent = title;
        titleEl.classList.remove('hidden');
      } else {
        titleEl.textContent = '';
        titleEl.classList.add('hidden');
      }
      document.getElementById('confirm-msg').textContent = msg;
      var okBtn = document.getElementById('confirm-ok');
      var cancelBtn = document.getElementById('confirm-cancel');
      okBtn.textContent = (opts && opts.okText) || 'Confirm';
      okBtn.className = 'btn ' + ((opts && opts.danger) ? 'danger' : 'primary');
      cancelBtn.style.display = '';
      overlay.classList.remove('hidden');
      var resolved = false;
      function done(val) {
        if (resolved) return;
        resolved = true;
        document.removeEventListener('keydown', onKey);
        overlay.removeEventListener('click', onBackdrop);
        overlay.classList.add('hidden');
        okBtn.replaceWith(okBtn.cloneNode(true));
        cancelBtn.replaceWith(cancelBtn.cloneNode(true));
        resolve(val);
      }
      function onBackdrop(e) { if (e.target === overlay) done(false); }
      function onKey(e) { if (e.key === 'Enter') done(true); else if (e.key === 'Escape') done(false); }
      overlay.addEventListener('click', onBackdrop);
      document.addEventListener('keydown', onKey, true);
      document.getElementById('confirm-ok').addEventListener('click', function() { done(true); });
      document.getElementById('confirm-cancel').addEventListener('click', function() { done(false); });
    });
  }

  // Structured review modal used for any "site is asking you to approve
  // something" flow — currently the extension bridge's sendTx and
  // signMessage requests, but the shape is general enough to reuse for
  // future action prompts too.
  //
  // Config:
  //   origin      (string, required) — site identifier shown in the badge
  //   title       (string, required) — big heading, e.g. "Send Transaction"
  //   subtitle    (string, optional) — dim one-liner under the title
  //   rows        (array,  optional) — list of
  //                 { label, value?, fbcBumps?, mono?, variant? }
  //               fbcBumps takes precedence: when set, the row value is
  //               rendered as <span class="fbc-icon"></span> + formatted
  //               number, matching the wallet's native amount display and
  //               dropping the literal "FBC" unit.
  //               variant is 'primary' or 'total' for the special rows.
  //   code        (string, optional) — monospace block for signMessage
  //   confirmText (string, required) — primary button label
  //   danger      (bool,   optional) — render primary as a danger button
  //
  // Returns a Promise that resolves to true (confirmed) or false (cancelled
  // / backdrop click / escape).
  function showReview(config) {
    return new Promise(function(resolve) {
      // ── DOM construction ──
      // Build the whole thing with createElement + textContent so there's
      // no innerHTML path where a caller's string could be interpreted as
      // markup. Origin strings, tx addresses, and raw sign-message content
      // all flow through user-controlled channels.

      var overlay = document.createElement('div');
      overlay.className = 'modal-overlay';

      var card = document.createElement('div');
      card.className = 'modal-card review';
      overlay.appendChild(card);

      // Header: origin badge + title + subtitle
      var header = document.createElement('div');
      header.className = 'review-header';
      card.appendChild(header);

      if (config.origin) {
        var badge = document.createElement('div');
        badge.className = 'review-origin';
        // Inline padlock glyph built via createElementNS (no innerHTML).
        // Kept inline so the modal is self-contained and matches the
        // currentColor of the badge.
        var svgNs = 'http://www.w3.org/2000/svg';
        var svg = document.createElementNS(svgNs, 'svg');
        svg.setAttribute('viewBox', '0 0 16 16');
        svg.setAttribute('fill', 'none');
        svg.setAttribute('stroke', 'currentColor');
        svg.setAttribute('stroke-width', '1.5');
        svg.setAttribute('stroke-linecap', 'round');
        svg.setAttribute('stroke-linejoin', 'round');
        var rect = document.createElementNS(svgNs, 'rect');
        rect.setAttribute('x', '3');
        rect.setAttribute('y', '7');
        rect.setAttribute('width', '10');
        rect.setAttribute('height', '7');
        rect.setAttribute('rx', '1.5');
        svg.appendChild(rect);
        var path = document.createElementNS(svgNs, 'path');
        path.setAttribute('d', 'M5 7V4.5a3 3 0 0 1 6 0V7');
        svg.appendChild(path);
        badge.appendChild(svg);

        var originLabel = document.createElement('span');
        originLabel.className = 'review-origin-label';
        originLabel.textContent = config.origin;
        badge.appendChild(originLabel);
        header.appendChild(badge);
      }

      if (config.title) {
        var title = document.createElement('div');
        title.className = 'review-title';
        title.textContent = config.title;
        header.appendChild(title);
      }
      if (config.subtitle) {
        var subtitle = document.createElement('div');
        subtitle.className = 'review-subtitle';
        subtitle.textContent = config.subtitle;
        header.appendChild(subtitle);
      }

      // Body: either rows (sendTx) or code block (signMessage). For request
      // types with no structured body at all (e.g. connect), skip the
      // div entirely so its bottom margin doesn't leave a dead gap.
      var hasBody = config.code !== undefined
        || (Array.isArray(config.rows) && config.rows.length > 0);
      var body = document.createElement('div');
      body.className = 'review-body';
      if (hasBody) card.appendChild(body);

      if (config.code !== undefined) {
        var code = document.createElement('div');
        code.className = 'review-code';
        code.textContent = config.code;
        body.appendChild(code);
      }
      if (Array.isArray(config.rows)) {
        config.rows.forEach(function(row) {
          var rowEl = document.createElement('div');
          rowEl.className = 'review-row' + (row.variant ? ' ' + row.variant : '');
          var label = document.createElement('div');
          label.className = 'review-row-label';
          label.textContent = row.label || '';
          var value = document.createElement('div');
          value.className = 'review-row-value'
            + (row.mono ? ' mono' : '')
            + (row.fbcBumps != null ? ' fbc' : '')
            + (row.stack ? ' stack' : '');

          if (row.fbcBumps != null) {
            // Fistbump native amount display: wordmark glyph (masked SVG
            // via the existing .fbc-icon class) followed by the formatted
            // number. Uses createElement + appendChild so we don't touch
            // innerHTML for a user-influenced value.
            var iconEl = document.createElement('span');
            iconEl.className = 'fbc-icon';
            value.appendChild(iconEl);
            var numEl = document.createElement('span');
            numEl.className = 'fbc-amount';
            numEl.textContent = formatFBC(row.fbcBumps, { noIcon: true });
            value.appendChild(numEl);
          } else if (row.stack) {
            // Two-line cell: an optional name on top, mono address below.
            // The .stack CSS hides the first span when :empty so callers
            // can unconditionally set both fields.
            var nameSpan = document.createElement('span');
            nameSpan.textContent = row.stack.name || '';
            value.appendChild(nameSpan);
            var addrSpan = document.createElement('span');
            addrSpan.textContent = row.stack.address || '';
            if (row.stack.address) addrSpan.title = row.stack.address;
            value.appendChild(addrSpan);
          } else {
            value.textContent = row.value != null ? String(row.value) : '';
            if (row.mono) value.title = String(row.value || '');
          }

          rowEl.appendChild(label);
          rowEl.appendChild(value);
          body.appendChild(rowEl);
        });
      }

      // Action row: Cancel + primary confirm
      var actions = document.createElement('div');
      actions.className = 'review-actions';
      var cancelBtn = document.createElement('div');
      cancelBtn.className = 'btn';
      cancelBtn.textContent = 'Cancel';
      var confirmBtn = document.createElement('div');
      confirmBtn.className = 'btn ' + (config.danger ? 'danger' : 'primary');
      confirmBtn.textContent = config.confirmText || 'Confirm';
      actions.appendChild(cancelBtn);
      actions.appendChild(confirmBtn);
      card.appendChild(actions);

      document.body.appendChild(overlay);

      // ── Event wiring ──
      var resolved = false;
      function done(val) {
        if (resolved) return;
        resolved = true;
        document.removeEventListener('keydown', onKey, true);
        overlay.remove();
        resolve(val);
      }
      function onKey(e) {
        if (e.key === 'Enter') { e.preventDefault(); done(true); }
        else if (e.key === 'Escape') { e.preventDefault(); done(false); }
      }
      confirmBtn.addEventListener('click', function() { done(true); });
      cancelBtn.addEventListener('click', function() { done(false); });
      overlay.addEventListener('click', function(e) {
        if (e.target === overlay) done(false);
      });
      document.addEventListener('keydown', onKey, true);
    });
  }

  function showAlert(msg, opts) {
    return new Promise(function(resolve) {
      var overlay = document.getElementById('confirm-modal');
      var titleEl = document.getElementById('confirm-title');
      var title = opts && opts.title;
      if (title) {
        titleEl.textContent = title;
        titleEl.classList.remove('hidden');
      } else {
        titleEl.textContent = '';
        titleEl.classList.add('hidden');
      }
      document.getElementById('confirm-msg').textContent = msg;
      var okBtn = document.getElementById('confirm-ok');
      var cancelBtn = document.getElementById('confirm-cancel');
      okBtn.textContent = (opts && opts.okText) || 'OK';
      cancelBtn.style.display = 'none';
      overlay.classList.remove('hidden');
      var resolved = false;
      function done() {
        if (resolved) return;
        resolved = true;
        document.removeEventListener('keydown', onKey, true);
        overlay.removeEventListener('click', onBackdrop);
        overlay.classList.add('hidden');
        okBtn.replaceWith(okBtn.cloneNode(true));
        cancelBtn.replaceWith(cancelBtn.cloneNode(true));
        resolve();
      }
      function onBackdrop(e) { if (e.target === overlay) done(); }
      function onKey(e) { if (e.key === 'Enter' || e.key === 'Escape') done(); }
      overlay.addEventListener('click', onBackdrop);
      document.addEventListener('keydown', onKey, true);
      document.getElementById('confirm-ok').addEventListener('click', function() { done(); });
    });
  }

  // ---- Browser Extension Bridge ----
  // The wallet runs a Unix socket listener at ~/.fistbump/extension.sock
  // that the bundled fistbump-bridge native messaging host forwards to.
  // When a dApp calls window.fistbump.{connect, sendTx, signMessage}, the
  // Rust side stashes the request and emits one of the events below. We
  // show the appropriate modal, do any fbd RPC work that's needed, and
  // ship the result back via `resolve_ext_request`, which unblocks the
  // listener thread.
  (function() {
    var ev = window.__TAURI__ && window.__TAURI__.event;
    if (!ev || typeof ev.listen !== 'function') return;

    // Shared helper: ack the Rust side with either a success payload or
    // an error string. Tauri rejects any command whose args miss a
    // required field, so we pass explicit nulls for the optional ones.
    async function resolveExt(id, result, error) {
      try {
        await __invoke('resolve_ext_request', {
          id: id,
          approve: !error,
          result: result || null,
          error: error || null,
        });
      } catch (err) {
        console.error('resolve_ext_request failed:', err);
      }
    }

    // ── connect ──
    // Ask the user whether to let the dApp see their address. On approve,
    // the Rust side builds the response itself (it knows the current
    // wallet's receive address) — we just say "yes" with no payload.
    ev.listen('ext://request', async function(e) {
      var p = e && e.payload;
      if (!p || !p.id || !p.origin) return;
      var ok = await showReview({
        origin: p.origin,
        title: 'Connect Wallet',
        subtitle: 'This site is requesting access to your Fistbump wallet. '
          + 'It will be able to see your address and ask to sign transactions and messages. '
          + 'Every transaction still requires your approval before it is signed.',
        confirmText: 'Connect',
      });
      await resolveExt(p.id, null, ok ? null : 'user denied');
    });

    // ── sendTx ──
    // The dApp asked us to send FBC to an address or Fistbump name. We
    // resolve the recipient, build the tx via createtx, show the user a
    // structured review modal with the fee-inclusive total, unlock the
    // wallet, sign, broadcast, and return the txid. Any RPC error
    // becomes a dApp-visible error.
    ev.listen('ext://tx-request', async function(e) {
      var p = e && e.payload;
      if (!p || !p.id || !p.origin || !p.to || typeof p.amount !== 'number') {
        if (p && p.id) await resolveExt(p.id, null, 'invalid tx request payload');
        return;
      }
      try {
        // Accept either an fb1... address or a Fistbump name in `to`.
        // resolveRecipient tries validateaddress first, then
        // resolveaddress for name lookups. When the input was a name, we
        // keep it around so the review modal can show it on top of the
        // resolved address.
        var resolved;
        try {
          resolved = await resolveRecipient(p.to);
        } catch (err) {
          await resolveExt(
            p.id,
            null,
            'could not resolve recipient: ' + ((err && err.message) || String(err))
          );
          return;
        }

        var createRes = await rpc('createtx', ['none', resolved.address, p.amount]);
        if (createRes.error) {
          await resolveExt(p.id, null, friendlyError(createRes.error));
          return;
        }
        var pstx = createRes.result.pstx;

        var decodeRes = await rpc('decoderawtransaction', [pstx]);
        if (decodeRes.error) {
          await resolveExt(p.id, null, friendlyError(decodeRes.error));
          return;
        }
        var fee = decodeRes.result.fee || 0;
        var amountDoo = Math.round(p.amount * 1000000);
        var totalDoo = amountDoo + fee;

        // Build the "To" row: if the dApp passed a Fistbump name, use
        // the stack variant to show name-on-top / address-below (same
        // treatment as the wallet's own Send review). Otherwise a plain
        // mono address cell.
        var toRow = resolved.name
          ? {
              label: 'To',
              stack: { name: resolved.name, address: resolved.address },
            }
          : { label: 'To', value: resolved.address, mono: true };

        var ok = await showReview({
          origin: p.origin,
          title: 'Send Transaction',
          subtitle: 'This site is requesting a transaction from your wallet.',
          rows: [
            { label: 'Amount', fbcBumps: amountDoo, variant: 'primary' },
            toRow,
            { label: 'Fee',    fbcBumps: fee },
            { label: 'Total',  fbcBumps: totalDoo, variant: 'total' },
          ],
          confirmText: 'Confirm & Send',
        });
        if (!ok) {
          await resolveExt(p.id, null, 'user denied');
          return;
        }

        // requireUnlock is a no-op if the wallet is already unlocked.
        if (!await requireUnlock()) {
          await resolveExt(p.id, null, 'unlock cancelled');
          return;
        }

        var signRes = await rpc('signtx', [pstx]);
        if (signRes.error) {
          await resolveExt(p.id, null, friendlyError(signRes.error));
          return;
        }
        var broadRes = await rpc('broadcasttx', [signRes.result.pstx]);
        if (broadRes.error) {
          await resolveExt(p.id, null, friendlyError(broadRes.error));
          return;
        }
        await resolveExt(p.id, { txid: broadRes.result.txid }, null);
      } catch (err) {
        await resolveExt(p.id, null, (err && err.message) || String(err));
      }
    });

    // ── signMessage ──
    // Pops a review modal with the message body in a monospace block,
    // unlocks the wallet, then calls either fbd's signmessage (against
    // the active wallet's receive address) or signmessagewithname (when
    // the dApp specified a Fistbump name in the payload). Returns
    // {signature, address} or {signature, name} accordingly.
    ev.listen('ext://sign-request', async function(e) {
      var p = e && e.payload;
      if (!p || !p.id || !p.origin || typeof p.message !== 'string') {
        if (p && p.id) await resolveExt(p.id, null, 'invalid sign request payload');
        return;
      }
      try {
        // When the dApp wants to sign as a specific Fistbump name, verify
        // ownership BEFORE popping the approval modal. A hostile dApp
        // could otherwise pass a name the user doesn't control and rely
        // on the "Signing as" label to mislead them. We ask fbd for the
        // current on-chain owner of the name, then check whether that
        // address belongs to this wallet via validateaddress.ismine —
        // the same pattern the name detail page uses.
        if (p.name) {
          var infoRes = await rpc('getnameinfo', [p.name]);
          if (infoRes.error) {
            await resolveExt(p.id, null, 'name lookup failed: ' + friendlyError(infoRes.error));
            return;
          }
          var nameInfo = infoRes.result;
          if (!nameInfo || !nameInfo.owner || !nameInfo.owner.address) {
            await resolveExt(p.id, null, 'the name "' + p.name + '" is not registered');
            return;
          }
          var ownerCheck = await rpc('validateaddress', [nameInfo.owner.address]);
          var ownerIsMine = ownerCheck.result && ownerCheck.result.ismine;
          if (!ownerIsMine) {
            await resolveExt(p.id, null, 'you don\u2019t own the name "' + p.name + '"');
            return;
          }
        }

        // Build review config. When the dApp asked to sign as a specific
        // Fistbump name, surface that prominently — the user should see
        // the signing identity before approving.
        var reviewConfig = {
          origin: p.origin,
          title: 'Sign Message',
          subtitle: p.name
            ? 'This site wants you to sign the following message as "' + p.name + '". It will not broadcast anything on-chain.'
            : 'This site wants you to sign the following message. It will not broadcast anything on-chain.',
          code: p.message,
          confirmText: 'Sign Message',
        };
        if (p.name) {
          reviewConfig.rows = [{ label: 'Signing as', value: p.name }];
        }

        var ok = await showReview(reviewConfig);
        if (!ok) {
          await resolveExt(p.id, null, 'user denied');
          return;
        }

        if (!await requireUnlock()) {
          await resolveExt(p.id, null, 'unlock cancelled');
          return;
        }

        if (p.name) {
          // signmessagewithname uses the private key owning the name,
          // not the wallet's default receive address.
          var signRes = await rpc('signmessagewithname', [p.name, p.message]);
          if (signRes.error) {
            await resolveExt(p.id, null, friendlyError(signRes.error));
            return;
          }
          await resolveExt(
            p.id,
            { signature: signRes.result, name: p.name },
            null
          );
        } else {
          var info = await rpc('getwalletinfo');
          if (info.error) {
            await resolveExt(p.id, null, friendlyError(info.error));
            return;
          }
          var address = info.result && info.result.address;
          if (!address) {
            await resolveExt(p.id, null, 'no active address');
            return;
          }
          var signByAddrRes = await rpc('signmessage', [address, p.message]);
          if (signByAddrRes.error) {
            await resolveExt(p.id, null, friendlyError(signByAddrRes.error));
            return;
          }
          await resolveExt(
            p.id,
            { signature: signByAddrRes.result, address: address },
            null
          );
        }
      } catch (err) {
        await resolveExt(p.id, null, (err && err.message) || String(err));
      }
    });

    // ── getPublicKey ──
    // Return the wallet's swap pubkey. No modal — the dApp already has the
    // user's address via connect(), and the pubkey commits to the same key
    // pair. This is purely a convenience so the dApp can build HTLC scripts
    // that include the wallet's pubkey in the right slots.
    ev.listen('ext://swap-pubkey-request', async function(e) {
      var p = e && e.payload;
      if (!p || !p.id || !p.origin) return;
      try {
        var res = await rpc('getswappubkey');
        if (res.error) {
          await resolveExt(p.id, null, friendlyError(res.error));
          return;
        }
        await resolveExt(
          p.id,
          { pubkey: res.result.pubkey, address: res.result.address },
          null
        );
      } catch (err) {
        await resolveExt(p.id, null, (err && err.message) || String(err));
      }
    });

    // ── fundHtlc ──
    // Verify the script is a canonical HTLC (via parsehtlcscript), show a
    // "Fund Swap" modal with amount + destination + hashlock + timelock,
    // then build/sign/broadcast the funding tx.
    ev.listen('ext://htlc-fund-request', async function(e) {
      var p = e && e.payload;
      if (!p || !p.id || !p.origin || !p.witnessScriptHex || typeof p.amount !== 'number') {
        if (p && p.id) await resolveExt(p.id, null, 'invalid HTLC fund request');
        return;
      }
      try {
        var parseRes = await rpc('parsehtlcscript', [p.witnessScriptHex]);
        if (parseRes.error || !parseRes.result) {
          await resolveExt(
            p.id,
            null,
            'witness script is not a valid HTLC — refusing to fund'
          );
          return;
        }
        var parsed = parseRes.result;
        var amountDoo = Math.round(p.amount * 1000000);

        var rows = [
          { label: 'Amount', fbcBumps: amountDoo, variant: 'primary' },
          { label: 'HTLC address', value: parsed.p2wsh_address, mono: true },
          { label: 'Hashlock', value: parsed.hashlock.slice(0, 16) + '...', mono: true },
          { label: 'Refund after block', value: Number(parsed.locktime).toLocaleString() },
        ];

        var ok = await showReview({
          origin: p.origin,
          title: 'Fund Swap',
          subtitle: 'This site is funding a cross-chain atomic swap. Your FBC will be '
            + 'locked until the counterparty claims it (revealing the preimage) or the '
            + 'timelock expires and you can refund.',
          rows: rows,
          confirmText: 'Fund Swap',
        });
        if (!ok) {
          await resolveExt(p.id, null, 'user denied');
          return;
        }
        if (!await requireUnlock()) {
          await resolveExt(p.id, null, 'unlock cancelled');
          return;
        }

        var fundRes = await rpc('createhtlcfund', [p.witnessScriptHex, p.amount]);
        if (fundRes.error) {
          await resolveExt(p.id, null, friendlyError(fundRes.error));
          return;
        }
        var pstx = fundRes.result.pstx;
        var signRes = await rpc('signtx', [pstx]);
        if (signRes.error) {
          await resolveExt(p.id, null, friendlyError(signRes.error));
          return;
        }
        var broadRes = await rpc('broadcasttx', [signRes.result.pstx]);
        if (broadRes.error) {
          await resolveExt(p.id, null, friendlyError(broadRes.error));
          return;
        }

        // The HTLC output is always vout 0 of the funding tx because
        // buildUnsignedHTLCFund emits the HTLC output first (change is
        // appended only when non-dust).
        await resolveExt(p.id, { txid: broadRes.result.txid, vout: 0 }, null);
      } catch (err) {
        await resolveExt(p.id, null, (err && err.message) || String(err));
      }
    });

    // ── signHtlcSpend ──
    // Sign a claim or refund spend of an HTLC output. Branch-specific
    // review modal. The wallet returns the signed raw tx hex and txid.
    // Note: this does NOT broadcast — the dApp decides when to broadcast
    // (claim should broadcast immediately; refund must wait for the
    // timelock height).
    ev.listen('ext://htlc-spend-request', async function(e) {
      var p = e && e.payload;
      if (!p || !p.id || !p.origin || !p.fundingTxid || !p.witnessScriptHex || !p.branch
          || !p.destinationAddress) {
        if (p && p.id) await resolveExt(p.id, null, 'invalid HTLC spend request');
        return;
      }
      try {
        var parseRes = await rpc('parsehtlcscript', [p.witnessScriptHex]);
        if (parseRes.error || !parseRes.result) {
          await resolveExt(p.id, null, 'witness script is not a valid HTLC');
          return;
        }
        var parsed = parseRes.result;

        var isClaim = p.branch === 'claim';
        var rows = [
          { label: 'Receive', fbcBumps: p.fundingAmount, variant: 'primary' },
          { label: 'To', value: p.destinationAddress, mono: true },
          { label: 'From HTLC', value: parsed.p2wsh_address, mono: true },
        ];
        if (isClaim && p.preimageHex) {
          rows.push({ label: 'Preimage', value: p.preimageHex.slice(0, 16) + '...', mono: true });
        }
        if (!isClaim) {
          rows.push({ label: 'Refund valid after block', value: Number(parsed.locktime).toLocaleString() });
        }

        var ok = await showReview({
          origin: p.origin,
          title: isClaim ? 'Claim Swap' : 'Refund Swap',
          subtitle: isClaim
            ? 'This site is claiming the counterparty\u2019s locked FBC by revealing '
              + 'the preimage. After broadcast, the counterparty will be able to see '
              + 'this preimage on-chain and use it to claim their side of the swap.'
            : 'This site is refunding an expired swap. Refund is only valid once the '
              + 'timelock has passed on-chain — if the block height above is in the future, '
              + 'the transaction will be rejected by the network until then.',
          rows: rows,
          confirmText: isClaim ? 'Claim' : 'Refund',
        });
        if (!ok) {
          await resolveExt(p.id, null, 'user denied');
          return;
        }
        if (!await requireUnlock()) {
          await resolveExt(p.id, null, 'unlock cancelled');
          return;
        }

        var rpcArgs = [
          p.fundingTxid,
          p.fundingVout,
          p.fundingAmount,
          p.witnessScriptHex,
          p.branch,
          p.destinationAddress,
          p.feeRate,
        ];
        if (isClaim) rpcArgs.push(p.preimageHex);

        var signRes = await rpc('signhtlcspend', rpcArgs);
        if (signRes.error) {
          await resolveExt(p.id, null, friendlyError(signRes.error));
          return;
        }
        await resolveExt(
          p.id,
          { rawTxHex: signRes.result.tx_hex, txid: signRes.result.txid },
          null
        );
      } catch (err) {
        await resolveExt(p.id, null, (err && err.message) || String(err));
      }
    });
  })();

  const NETWORKS = {
    main:    { rpcPort: 32869, hrp: 'fb' },
    testnet: { rpcPort: 42869, hrp: 'ft' },
    regtest: { rpcPort: 52869, hrp: 'fr' },
    simnet:  { rpcPort: 62869, hrp: 'fs' },
  };
  let network = 'testnet'; // overwritten by get_settings at startup
  const EXPLORER = 'https://explorer.fistbump.org';
  let activeWallet = null;
  let walletEncrypted = false;
  let walletUnlocked = true;
  let walletIsMultisig = false;
  let walletMultisigM = 0;
  let walletMultisigN = 0;
  let biometricSupported = false;

  // Switch between login pages — fade the whole .login-card.
  var loginPages = document.querySelector('.login-pages');
  var loginCard = document.querySelector('#login-screen .login-card');
  // Step-based fade: set opacity in small steps via requestAnimationFrame.
  // Yields two frames first so the browser renders current state before animating.
  function stepFade(el, from, to, duration) {
    return new Promise(resolve => {
      requestAnimationFrame(() => requestAnimationFrame(() => {
        var start = null;
        function frame(ts) {
          if (!start) start = ts;
          var t = Math.min((ts - start) / duration, 1);
          el.style.opacity = String(from + (to - from) * t);
          if (t < 1) requestAnimationFrame(frame);
          else resolve();
        }
        el.style.opacity = String(from);
        requestAnimationFrame(frame);
      }));
    });
  }
  async function fadeCardOut() {
    await stepFade(loginCard, 1, 0, 150);
  }
  async function fadeCardIn() {
    await stepFade(loginCard, 0, 1, 150);
    loginCard.style.opacity = '';
  }
  async function showLoginPage(pageId, beforeShow) {
    await fadeCardOut();
    loginPages.querySelectorAll('.login-page').forEach(p => p.classList.add('hidden'));
    if (beforeShow) await beforeShow();
    document.getElementById(pageId).classList.remove('hidden');
    await fadeCardIn();
  }

  function showToast(title, txid) {
    var container = document.getElementById('toast-container');
    var el = document.createElement('div');
    el.className = 'toast';
    var shortHash = txid ? (txid.slice(0, 10) + '...' + txid.slice(-6)) : '';
    el.innerHTML = '<div class="toast-icon">\u2713</div>' +
      '<div class="toast-body"><div class="toast-title">' + esc(title) + '</div>' +
      (txid ? '<div class="toast-hash">' + esc(shortHash) + '</div>' : '') + '</div>' +
      (txid ? '<a class="toast-btn" href="' + EXPLORER + '/tx/' + esc(txid) + '" target="_blank">View</a>' : '');
    container.appendChild(el);
    setTimeout(function() {
      el.classList.add('toast-out');
      el.addEventListener('animationend', function() { el.remove(); });
    }, 5000);
  }

  function blockLink(height) {
    if (!height && height !== 0) return '--';
    return '<a href="' + EXPLORER + '/block/' + height + '" target="_blank" class="block-link">' + Number(height).toLocaleString() + '</a>';
  }

  function nameLink(name, nameHash) {
    if (name) return '<a href="#" class="name-link" data-name="' + esc(name) + '">' + esc(name) + '</a>';
    return '<span class="mono-sm">' + esc(nameHash.slice(0, 16)) + '...</span>';
  }

  function esc(str) {
    const d = document.createElement('div');
    d.textContent = str;
    return d.innerHTML;
  }

  // RPC via Tauri invoke.
  // Automatically prompts for unlock if the server returns "wallet is locked".
  async function rpc(method, params, opts) {
    const w = (opts && opts.wallet) || activeWallet;
    var res = await __invoke('rpc_call', {
      method: method,
      params: params || [],
      wallet: w || null
    });
    if (res.error && typeof res.error === 'string' && res.error.toLowerCase().indexOf('wallet is locked') >= 0
        && method !== 'walletpassphrase') {
      walletUnlocked = false;
      updateLockIndicator();
      var ok = await requireUnlock();
      if (!ok) return res;
      // Retry the call after unlock
      return __invoke('rpc_call', {
        method: method,
        params: params || [],
        wallet: w || null
      });
    }
    return res;
  }

  // Prompt user to unlock if wallet is encrypted and locked.
  // Tries biometric first if available and enabled, then falls back to passphrase input.
  // Returns true if wallet is unlocked (or was just unlocked), false if cancelled.
  async function requireUnlock() {
    if (!walletEncrypted || walletUnlocked) return true;

    // Try biometric unlock first.
    if (biometricSupported && activeWallet && localStorage.getItem('biometric_' + activeWallet) === '1') {
      var loadedOk = false;
      try {
        var passphrase = await __invoke('biometric_load', { wallet: activeWallet });
        if (passphrase) {
          loadedOk = true;
          var res = await rpc('walletpassphrase', [passphrase, 300]);
          if (!res.error) {
            walletUnlocked = true;
            updateLockIndicator();
            return true;
          }
        }
      } catch(e) {
        // Biometric failed or was cancelled by the user.
      }
      // If biometric_load returned nothing (empty Keychain entry — e.g. after
      // upgrading from the legacy file-based store, or after the user reset
      // Touch ID enrollment and the entry was invalidated), clear the local
      // flag so offerBiometricSetup will re-offer after a successful manual
      // unlock instead of silently staying in "enabled but broken" mode.
      if (!loadedOk) {
        localStorage.removeItem('biometric_' + activeWallet);
      }
    }

    // Fall back to passphrase input
    return new Promise(function(resolve) {
      var overlay = document.getElementById('unlock-modal');
      var input = document.getElementById('unlock-passphrase');
      var okBtn = document.getElementById('unlock-ok');
      var cancelBtn = document.getElementById('unlock-cancel');
      var statusEl = document.getElementById('unlock-status');
      input.value = '';
      statusEl.innerHTML = '';
      okBtn.style.opacity = '';
      okBtn.style.pointerEvents = '';
      overlay.classList.remove('hidden');
      setTimeout(function() { input.focus(); }, 50);
      var resolved = false;
      function done(ok) {
        if (resolved) return;
        resolved = true;
        okBtn.style.opacity = '';
        okBtn.style.pointerEvents = '';
        overlay.classList.add('hidden');
        document.removeEventListener('keydown', onKey, true);
        okBtn.replaceWith(okBtn.cloneNode(true));
        cancelBtn.replaceWith(cancelBtn.cloneNode(true));
        resolve(ok);
      }
      async function tryUnlock() {
        var passphrase = input.value;
        if (!passphrase) {
          statusEl.innerHTML = '<div class="error-msg">Enter a passphrase.</div>';
          return;
        }
        okBtn.style.opacity = '0.5';
        okBtn.style.pointerEvents = 'none';
        statusEl.innerHTML = '<div class="modal-loading">Unlocking...</div>';
        try {
          var res = await rpc('walletpassphrase', [passphrase, 300]);
          if (res.error) throw new Error(res.error);
          walletUnlocked = true;
          updateLockIndicator();
          done(true);
          // Offer to enable biometric after successful manual unlock
          offerBiometricSetup(passphrase);
        } catch(e) {
          statusEl.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          okBtn.style.opacity = '';
          okBtn.style.pointerEvents = '';
          input.value = '';
          input.focus();
        }
      }
      function onKey(e) {
        if (e.key === 'Enter') { e.preventDefault(); tryUnlock(); }
        else if (e.key === 'Escape') done(false);
      }
      document.addEventListener('keydown', onKey, true);
      document.getElementById('unlock-ok').addEventListener('click', tryUnlock);
      document.getElementById('unlock-cancel').addEventListener('click', function() { done(false); });
    });
  }

  // After a successful manual unlock, offer to store the passphrase behind biometrics.
  async function offerBiometricSetup(passphrase) {
    if (!biometricSupported || !activeWallet) return;
    // Don't offer if already enabled or previously declined
    if (localStorage.getItem('biometric_' + activeWallet) !== null) return;
    var ok = await showConfirm('Enable biometric unlock for this wallet?');
    if (!ok) {
      // Mark as declined so we don't ask again every unlock.
      localStorage.setItem('biometric_' + activeWallet, '0');
      return;
    }
    var wallet = activeWallet;
    try {
      var saved = await __invoke('biometric_save', { wallet: wallet, passphrase: passphrase });
      if (saved) {
        localStorage.setItem('biometric_' + wallet, '1');
        showToast('Biometric unlock enabled');
        // Refresh the Wallet Management page so the Enable/Disable buttons
        // reflect the new state instead of still showing "Enable".
        if (activeWallet === wallet) loadWalletInfo();
      } else {
        localStorage.setItem('biometric_' + wallet, '0');
        await showAlert('Could not enable biometric unlock.');
      }
    } catch(e) {
      var msg = (typeof e === 'string') ? e : (e && e.message) ? e.message : String(e);
      console.error('biometric_save failed:', e);
      localStorage.setItem('biometric_' + wallet, '0');
      await showAlert('Could not enable biometric unlock: ' + msg);
    }
  }

  // After wallet creation/import, offer to encrypt with a passphrase.
  async function offerEncryptAfterCreate() {
    if (!activeWallet) return;
    if (!await showConfirm('Set a passphrase to protect your wallet?')) return;
    var pass1 = await showPassphrasePrompt('Choose a passphrase');
    if (!pass1) return;
    var pass2 = await showPassphrasePrompt('Confirm passphrase');
    if (!pass2) return;
    if (pass1 !== pass2) {
      await showAlert('Passphrases do not match.');
      return;
    }
    try {
      var res = await rpc('encryptwallet', [pass1]);
      if (res.error) throw new Error(res.error);
      walletEncrypted = true;
      walletUnlocked = false;
      updateLockIndicator();
      // Immediately unlock so the user doesn't have to type it again
      var unlockRes = await rpc('walletpassphrase', [pass1, 300]);
      if (!unlockRes.error) {
        walletUnlocked = true;
        updateLockIndicator();
        offerBiometricSetup(pass1);
      }
    } catch(e) {
      showAlert(friendlyError(e.message));
    }
  }

  // Reusable passphrase prompt (iOS-safe, no native prompt()).
  function showPassphrasePrompt(msg) {
    return new Promise(function(resolve) {
      var overlay = document.getElementById('unlock-modal');
      var input = document.getElementById('unlock-passphrase');
      var okBtn = document.getElementById('unlock-ok');
      var cancelBtn = document.getElementById('unlock-cancel');
      var statusEl = document.getElementById('unlock-status');
      var msgEl = overlay.querySelector('.confirm-dialog-msg');
      msgEl.textContent = msg || 'Enter passphrase';
      okBtn.textContent = 'OK';
      okBtn.style.opacity = '';
      okBtn.style.pointerEvents = '';
      input.value = '';
      statusEl.innerHTML = '';
      overlay.classList.remove('hidden');
      setTimeout(function() { input.focus(); }, 50);
      var resolved = false;
      function done(val) {
        if (resolved) return;
        resolved = true;
        msgEl.textContent = 'Enter passphrase to unlock wallet';
        okBtn.textContent = 'Unlock';
        overlay.classList.add('hidden');
        document.removeEventListener('keydown', onKey, true);
        okBtn.replaceWith(okBtn.cloneNode(true));
        cancelBtn.replaceWith(cancelBtn.cloneNode(true));
        resolve(val);
      }
      function onKey(e) {
        if (e.key === 'Enter') { e.preventDefault(); done(input.value || null); }
        else if (e.key === 'Escape') done(null);
      }
      document.addEventListener('keydown', onKey, true);
      document.getElementById('unlock-ok').addEventListener('click', function() { done(input.value || null); });
      document.getElementById('unlock-cancel').addEventListener('click', function() { done(null); });
    });
  }

  function updateLockIndicator() {
    var lockEl = document.getElementById('btn-lock-wallet');
    var encryptEl = document.getElementById('btn-encrypt-wallet');
    var infoEl = document.getElementById('wallet-info-encryption');
    if (!walletEncrypted) {
      if (infoEl) infoEl.textContent = 'Off';
      if (encryptEl) encryptEl.classList.remove('hidden');
      if (lockEl) lockEl.classList.add('hidden');
    } else if (walletUnlocked) {
      if (infoEl) infoEl.textContent = 'Encrypted (unlocked)';
      if (encryptEl) encryptEl.classList.add('hidden');
      if (lockEl) { lockEl.classList.remove('hidden'); lockEl.textContent = 'Lock Wallet'; }
    } else {
      if (infoEl) infoEl.textContent = 'Encrypted (locked)';
      if (encryptEl) encryptEl.classList.add('hidden');
      if (lockEl) { lockEl.classList.remove('hidden'); lockEl.textContent = 'Unlock Wallet'; }
    }
  }

  // Format an error message for display.
  // Server errors already have good descriptions; this only handles client-side errors.
  function friendlyError(msg) {
    if (!msg) return 'Something went wrong.';
    var s = typeof msg === 'string' ? msg : msg.toString();
    if (s.includes('Connection refused') || s.includes('connect ECONNREFUSED')) return 'Cannot connect to the node. Is it running?';
    if (s.includes('timeout') || s.includes('timed out')) return 'Request timed out. The node may be busy.';
    if (s.includes('wallet is locked')) return 'Wallet is locked. Unlock it first.';
    if (s.includes('wallet not initialized')) return 'No wallet loaded.';
    if (s.includes('no valid operations parsed')) return 'Nothing to do — no eligible names found.';
    if (s.includes('failed to derive bid nonce')) return 'Failed to generate bid secret. Try again.';
    if (s.includes('malformed TRANSFER covenant')) return 'Invalid transfer data on this name.';
    if (s.includes('cannot find block for renewal proof')) return 'Node is still syncing. Try again later.';
    if (s.includes('insufficient funds')) return 'Not enough funds for this transaction.';
    if (s.includes('Insufficient balance')) return 'Not enough funds for this transaction.';
    // Clean up state references: "(state: opening)" → "(currently in opening phase)"
    s = s.replace(/\(state: (\w+)\)/gi, function(_, st) { return '(currently in ' + st.toLowerCase() + ' phase)'; });
    // Strip "Invalid covenant: " prefix
    s = s.replace(/^Invalid covenant:\s*/i, '');
    // Capitalize first letter
    if (s.length > 0) s = s.charAt(0).toUpperCase() + s.slice(1);
    return s;
  }

  // ---- Login Screen ----

  async function initLogin() {
    const status = document.getElementById('login-status');
    const listEl = document.getElementById('wallet-list');
    const actions = document.getElementById('login-actions');

    listEl.innerHTML = '';
    listEl.style.display = 'none';
    actions.style.display = 'none';
    status.textContent = '';

    let connected = false;
    try {
      const res = await rpc('getblockchaininfo', [], { wallet: null });
      if (!res.error) connected = true;
    } catch(e) {}
    if (!connected) {
      status.textContent = 'Connecting to node...';
      for (let i = 0; i < 60; i++) {
        await new Promise(r => setTimeout(r, 1000));
        try {
          const res = await rpc('getblockchaininfo', [], { wallet: null });
          if (!res.error) { connected = true; break; }
        } catch(e) {}
      }
    }
    if (!connected) {
      status.textContent = 'Could not connect to node.';
      return;
    }

    const res = await rpc('listwallets', [], { wallet: null });
    if (res.error) { status.textContent = friendlyError(res.error); return; }
    const wallets = res.result || [];

    if (wallets.length === 0) {
      status.textContent = 'No wallets found. Create one to get started.';
      actions.style.display = '';
      return;
    }

    status.textContent = 'Select a wallet';
    listEl.innerHTML = wallets.map(name =>
      '<div class="wallet-item" data-name="' + esc(name) + '">' +
        '<span>' + esc(name) + '</span>' +
        '<span class="wallet-arrow">\u203A</span>' +
      '</div>'
    ).join('');
    listEl.style.display = '';
    actions.style.display = '';

    listEl.querySelectorAll('.wallet-item').forEach(el => {
      el.addEventListener('click', () => selectWallet(el.dataset.name));
    });
  }

  async function selectWallet(name) {
    activeWallet = name;
    // Let the Rust side know which wallet to use for browser-extension
    // requests. Best-effort — desktop only, missing on mobile.
    try { __invoke('set_active_wallet', { name: name }); } catch(_) {}
    var loginEl = document.getElementById('login-screen');
    var appEl = document.getElementById('app');
    // Fade out login screen
    loginEl.style.opacity = '0';
    await new Promise(r => setTimeout(r, 150));
    // Clear stale content
    document.getElementById('sidebar-wallet-name').textContent = name;
    document.getElementById('my-names').innerHTML = '';
    document.getElementById('bid-list').innerHTML = '';
    document.getElementById('auction-stats').innerHTML = '';
    document.getElementById('tx-list').innerHTML = '';
    document.getElementById('balance').innerHTML = '-- ';
    document.getElementById('balance-details').innerHTML = '';
    var encStatus = document.getElementById('encrypt-status');
    if (encStatus) encStatus.innerHTML = '';
    var walletInfo = document.getElementById('wallet-info');
    if (walletInfo) walletInfo.innerHTML = '';
    // Reset to overview tab
    document.querySelectorAll('.nav-item').forEach(b => b.classList.remove('active'));
    document.querySelectorAll('.page').forEach(t => t.classList.remove('active'));
    var overviewBtn = document.querySelector('.nav-item[data-tab="overview"]');
    if (overviewBtn) overviewBtn.classList.add('active');
    document.getElementById('tab-overview').classList.add('active');
    // Swap screens
    loginEl.classList.add('hidden');
    loginEl.style.opacity = '';
    appEl.style.opacity = '0';
    appEl.classList.remove('hidden');
    connectEventStream(); // reconnect SSE with wallet filter
    // Check biometric support and wallet type
    try { biometricSupported = await __invoke('biometric_available'); } catch(e) { biometricSupported = false; }
    try {
      var winfo = await rpc('getwalletinfo');
      if (winfo.result) {
        walletIsMultisig = winfo.result.type === 'multisig';
        walletMultisigM = winfo.result.m || 0;
        walletMultisigN = winfo.result.n || 0;
      }
    } catch(e) {}
    // Show Cosign button for multisig wallets
    var cosignBtn = document.getElementById('btn-show-cosign');
    if (cosignBtn) cosignBtn.classList.toggle('hidden', !walletIsMultisig);
    await refresh();
    appEl.style.opacity = '1';
  }

  async function doLogout() {
    if (walletEncrypted && walletUnlocked) {
      try { await rpc('walletlock'); } catch(e) {}
      walletUnlocked = false;
      updateLockIndicator();
    }
    activeWallet = null;
    try { __invoke('set_active_wallet', { name: null }); } catch(_) {}
    currentAddress = null;
    walletIsMultisig = false;
    walletMultisigM = 0;
    walletMultisigN = 0;
    var cosignBtn = document.getElementById('btn-show-cosign');
    if (cosignBtn) cosignBtn.classList.add('hidden');
    searchInput.value = '';
    updateSearchClear();
    var appEl = document.getElementById('app');
    var loginEl = document.getElementById('login-screen');
    // Fade out app while loading wallet list in the background
    loginPages.querySelectorAll('.login-page').forEach(p => p.classList.add('hidden'));
    document.getElementById('login-page-main').classList.remove('hidden');
    loginEl.style.opacity = '0';
    loginEl.classList.remove('hidden');
    var loginReady = initLogin();
    await stepFade(appEl, 1, 0, 150);
    appEl.classList.add('hidden');
    appEl.style.opacity = '';
    // Wait for wallet list, then fade in login
    await loginReady;
    await stepFade(loginEl, 0, 1, 150);
    loginEl.style.opacity = '';
  }

  document.getElementById('btn-logout').addEventListener('click', function() { closeSidebar(); doLogout(); });

  // Mobile-only buttons
  var logoutMobile = document.getElementById('btn-logout-mobile');
  if (logoutMobile) logoutMobile.addEventListener('click', doLogout);

  var logMobile = document.getElementById('btn-show-log-mobile');
  if (logMobile) logMobile.addEventListener('click', function() {
    document.querySelectorAll('.nav-item').forEach(function(b) { b.classList.remove('active'); });
    document.querySelectorAll('.page').forEach(function(t) { t.classList.remove('active'); });
    document.getElementById('tab-log').classList.add('active');
    refreshLog();
  });

  // Create / Import wallet
  let isImporting = false;

  document.getElementById('btn-create-wallet').addEventListener('click', async () => {
    isImporting = false;
    document.getElementById('new-wallet-name').value = '';
    document.getElementById('import-field').classList.add('hidden');
    document.getElementById('btn-login-submit').textContent = 'Create';
    document.getElementById('login-form-status').innerHTML = '';
    await showLoginPage('login-page-create');
  });

  document.getElementById('btn-import-wallet').addEventListener('click', async () => {
    isImporting = true;
    document.getElementById('new-wallet-name').value = '';
    const grid = document.getElementById('import-grid');
    grid.innerHTML = Array.from({length: 24}, (_, i) =>
      '<div class="mnemonic-input-wrap">' +
        '<span class="word-num">' + (i + 1) + '</span>' +
        '<input type="text" class="import-word" data-idx="' + i + '" autocomplete="off" autocapitalize="off" spellcheck="false" value="">' +
      '</div>'
    ).join('');
    // Ensure inputs are cleared (some browsers restore values despite innerHTML replacement)
    grid.querySelectorAll('.import-word').forEach(function(el) { el.value = ''; });
    document.getElementById('import-field').classList.remove('hidden');
    document.getElementById('btn-login-submit').textContent = 'Import';
    document.getElementById('login-form-status').innerHTML = '';
    await showLoginPage('login-page-create');
    grid.querySelectorAll('.import-word').forEach(input => {
      input.addEventListener('keydown', (e) => {
        if (e.key === ' ') {
          e.preventDefault();
          const next = grid.querySelector('[data-idx="' + (parseInt(input.dataset.idx) + 1) + '"]');
          if (next) next.focus();
        }
      });
      input.addEventListener('paste', (e) => {
        const text = (e.clipboardData || window.clipboardData).getData('text').trim();
        const words = text.split(/\s+/);
        if (words.length > 1) {
          e.preventDefault();
          words.forEach((w, j) => {
            const field = grid.querySelector('[data-idx="' + (parseInt(input.dataset.idx) + j) + '"]');
            if (field) field.value = w;
          });
        }
      });
    });
  });

  // ---- Multisig Wallet Creation ----

  document.getElementById('btn-create-multisig').addEventListener('click', async () => {
    document.getElementById('ms-wallet-name').value = '';
    document.getElementById('ms-m').value = '';
    document.getElementById('ms-xpubs').innerHTML =
      '<textarea class="ms-xpub-input" placeholder="Cosigner 1 xpub..." autocomplete="off" autocorrect="off" spellcheck="false"></textarea>';
    document.getElementById('ms-form-status').innerHTML = '';

    // Populate key source dropdown with existing wallets
    var select = document.getElementById('ms-key-source');
    select.innerHTML = '<option value="new">Generate new key</option>';
    try {
      var res = await rpc('listwallets', [], { wallet: null });
      if (res.result && Array.isArray(res.result)) {
        res.result.forEach(function(name) {
          var opt = document.createElement('option');
          opt.value = name;
          opt.textContent = 'Use key from "' + name + '"';
          select.appendChild(opt);
        });
      }
    } catch(e) {}

    await showLoginPage('login-page-multisig');
  });

  document.getElementById('btn-ms-add-xpub').addEventListener('click', () => {
    var container = document.getElementById('ms-xpubs');
    var count = container.querySelectorAll('.ms-xpub-input').length;
    var ta = document.createElement('textarea');
    ta.className = 'ms-xpub-input';
    ta.placeholder = 'Cosigner ' + (count + 1) + ' xpub...';
    ta.autocomplete = 'off';
    ta.spellcheck = false;
    container.appendChild(ta);
  });

  document.getElementById('btn-ms-back').addEventListener('click', async () => {
    await showLoginPage('login-page-main', initLogin);
  });

  document.getElementById('btn-ms-submit').addEventListener('click', async () => {
    var name = document.getElementById('ms-wallet-name').value.trim();
    var m = parseInt(document.getElementById('ms-m').value);
    var el = document.getElementById('ms-form-status');
    if (!name) { el.innerHTML = '<div class="error-msg">Enter a wallet name.</div>'; return; }
    if (!m || m < 1) { el.innerHTML = '<div class="error-msg">Enter a valid number of required signatures.</div>'; return; }
    var xpubs = Array.from(document.querySelectorAll('.ms-xpub-input'))
      .map(function(ta) { return ta.value.trim(); })
      .filter(function(v) { return v.length > 0; });
    if (xpubs.length === 0) { el.innerHTML = '<div class="error-msg">Add at least one cosigner xpub.</div>'; return; }
    var n = xpubs.length + 1; // cosigners + this wallet
    if (m > n) { el.innerHTML = '<div class="error-msg">Required signatures (' + m + ') cannot exceed total signers (' + n + ').</div>'; return; }

    var keySource = document.getElementById('ms-key-source').value;

    // If using an existing wallet's key, check if it needs unlocking
    if (keySource !== 'new') {
      try {
        var infoRes = await rpc('getwalletinfo', [], { wallet: keySource });
        if (infoRes.result && infoRes.result.encrypted && !infoRes.result.unlocked) {
          // Try biometric unlock first
          var bioAvail = false;
          try { bioAvail = await __invoke('biometric_available'); } catch(e) {}
          if (bioAvail && localStorage.getItem('biometric_' + keySource) === '1') {
            try {
              var bioPass = await __invoke('biometric_load', { wallet: keySource });
              if (bioPass) {
                var bioRes = await rpc('walletpassphrase', [bioPass, 30], { wallet: keySource });
                if (!bioRes.error) {
                  // Biometric unlock succeeded
                }
              }
            } catch(e) {}
            // Re-check if it's unlocked now
            var recheck = await rpc('getwalletinfo', [], { wallet: keySource });
            if (recheck.result && recheck.result.unlocked) {
              // Good, proceed
            } else {
              // Fall through to manual prompt
            }
          }

          // Check again in case biometric succeeded
          var recheckInfo = await rpc('getwalletinfo', [], { wallet: keySource });
          if (recheckInfo.result && !recheckInfo.result.unlocked) {
          // Manual passphrase prompt
          var passphrase = await new Promise(function(resolve) {
            var overlay = document.getElementById('unlock-modal');
            var input = document.getElementById('unlock-passphrase');
            var okBtn = document.getElementById('unlock-ok');
            var cancelBtn = document.getElementById('unlock-cancel');
            var unlockStatus = document.getElementById('unlock-status');
            input.value = '';
            unlockStatus.innerHTML = '<div class="muted text-base">Unlock "' + esc(keySource) + '" to use its key.</div>';
            overlay.classList.remove('hidden');
            setTimeout(function() { input.focus(); }, 50);
            var resolved = false;
            function done(pw) { if (resolved) return; resolved = true; overlay.classList.add('hidden'); document.removeEventListener('keydown', onKey, true); okBtn.replaceWith(okBtn.cloneNode(true)); cancelBtn.replaceWith(cancelBtn.cloneNode(true)); resolve(pw); }
            async function tryVerify() {
              var pw = input.value;
              if (!pw) { unlockStatus.innerHTML = '<div class="error-msg">Enter a passphrase.</div>'; return; }
              okBtn.style.opacity = '0.5'; okBtn.style.pointerEvents = 'none';
              unlockStatus.innerHTML = '<div class="modal-loading">Unlocking...</div>';
              try {
                var r = await rpc('walletpassphrase', [pw, 30], { wallet: keySource });
                if (r.error) throw new Error(r.error);
                done(pw);
              } catch(e) {
                unlockStatus.innerHTML = '<div class="error-msg">Wrong passphrase.</div>';
                okBtn.style.opacity = ''; okBtn.style.pointerEvents = '';
                input.value = ''; input.focus();
              }
            }
            function onKey(e) { if (e.key === 'Enter') { e.preventDefault(); tryVerify(); } else if (e.key === 'Escape') done(null); }
            document.addEventListener('keydown', onKey, true);
            document.getElementById('unlock-ok').addEventListener('click', tryVerify);
            document.getElementById('unlock-cancel').addEventListener('click', function() { done(null); });
          });
          if (!passphrase) { el.innerHTML = ''; return; }
          }
        }
      } catch(e) {}
    }

    el.innerHTML = '<div class="modal-loading">Creating multisig wallet...</div>';
    try {
      var opts = keySource !== 'new' ? { wallet: keySource } : { wallet: null };
      var res = await rpc('createmultisigwallet', [name, m, xpubs.join(',')], opts);
      if (res.error) throw new Error(res.error);
      var result = res.result;
      // Fade out, swap to success content, fade in
      await fadeCardOut();

      var msPage = document.getElementById('login-page-multisig');
      msPage.querySelectorAll(':scope > .fb-list, :scope > .section-label, #ms-xpubs, #btn-ms-add-xpub, :scope > .confirm-actions').forEach(function(el) {
        el.style.display = 'none';
      });

      var xpubHtml = '';
      var xpubValue = result.xpub || '';
      if (xpubValue) {
        xpubHtml = '<div class="section-label">Your Extended Public Key</div>' +
          '<div class="ms-xpub-display xpub-box">' +
          esc(xpubValue) + '</div>' +
          '<div class="btn" id="btn-ms-copy-xpub">Copy xpub</div>' +
          '<div class="mnemonic-warning">Share this extended public key (xpub) with your cosigners. They will need it to create their multisig wallet.</div>';
      }
      el.innerHTML = '<div class="flex-col gap-12">' +
        '<div class="success-msg">Multisig wallet created.</div>' + xpubHtml +
        '<div class="btn primary" id="btn-ms-done">Done</div>' +
        '</div>';

      await fadeCardIn();

      if (xpubValue) {
        document.getElementById('btn-ms-copy-xpub').addEventListener('click', function() {
          navigator.clipboard.writeText(xpubValue).then(function() {
            showToast('Copied to clipboard');
          });
        });
      }
      document.getElementById('btn-ms-done').addEventListener('click', async function() {
        await showLoginPage('login-page-main', initLogin);
        // Restore hidden form fields for next time (page is hidden now, safe to reset)
        document.querySelectorAll('#login-page-multisig [style*="display: none"]').forEach(function(el) {
          el.style.display = '';
        });
      });
    } catch(e) {
      el.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  });

  document.getElementById('btn-login-back').addEventListener('click', async () => {
    document.getElementById('new-wallet-name').value = '';
    await showLoginPage('login-page-main', initLogin);
  });

  document.getElementById('btn-login-submit').addEventListener('click', async () => {
    const name = document.getElementById('new-wallet-name').value.trim();
    const el = document.getElementById('login-form-status');
    if (!name) {
      el.innerHTML = '<div class="error-msg">Enter a wallet name.</div>';
      return;
    }
    const params = [name];
    if (isImporting) {
      const words = Array.from(document.querySelectorAll('.import-word')).map(i => i.value.trim().toLowerCase());
      const phrase = words.join(' ');
      if (words.some(w => !w)) {
        el.innerHTML = '<div class="error-msg">Fill in all 24 words.</div>';
        return;
      }
      params.push(phrase);
    }
    el.innerHTML = '<div class="modal-loading">Creating wallet...</div>';
    try {
      const res = await rpc('createwallet', params, { wallet: null });
      if (res.error) throw new Error(res.error);
      const result = res.result;

      if (result.mnemonic && !isImporting) {
        const words = result.mnemonic.split(' ');
        document.getElementById('mnemonic-raw').value = result.mnemonic;
        document.getElementById('mnemonic-words').innerHTML = words.map((w, i) =>
          '<span class="mnemonic-word"><span class="word-num">' + (i + 1) + '</span>' + esc(w) + '</span>'
        ).join('');
        await showLoginPage('login-page-mnemonic');
        document.getElementById('btn-copy-mnemonic').addEventListener('click', () => {
          navigator.clipboard.writeText(result.mnemonic);
          const b = document.getElementById('btn-copy-mnemonic');
          b.textContent = 'Copied';
          setTimeout(() => { b.textContent = 'Copy to Clipboard'; }, 1500);
        });
        document.getElementById('btn-mnemonic-done').addEventListener('click', async () => {
          await selectWallet(result.name);
          offerEncryptAfterCreate();
        });
      } else {
        await selectWallet(result.name);
        offerEncryptAfterCreate();
      }
    } catch(e) {
      el.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  });

  // Tab switching
  document.querySelectorAll('.nav-item[data-tab]').forEach(btn => {
    btn.addEventListener('click', async () => {
      closeSidebar();
      // Browse: fire the modal native browser instead of switching tabs.
      if (btn.dataset.tab === 'browse') {
        openBrowser();
        return;
      }
      // If clicking the already-active tab, refresh data without transition
      if (btn.classList.contains('active') && !currentNameDetail) {
        try {
          if (btn.dataset.tab === 'overview') refreshOverviewActions();
          if (btn.dataset.tab === 'log') refreshLog();
          if (btn.dataset.tab === 'wallet') await loadWalletInfo();
          if (btn.dataset.tab === 'names') await loadMyNames();
          if (btn.dataset.tab === 'auctions') await loadAuctions();
          if (btn.dataset.tab === 'transactions') await loadAllTransactions(true);
        } catch(e) {}
        document.querySelector('main').scrollTop = 0;
        return;
      }
      currentNameDetail = null;
      var main = document.querySelector('main');
      main.style.opacity = '0';
      await new Promise(r => setTimeout(r, 100));
      main.scrollTop = 0;
      document.querySelectorAll('.nav-item').forEach(b => b.classList.remove('active'));
      document.querySelectorAll('.page').forEach(t => t.classList.remove('active'));
      btn.classList.add('active');
      document.getElementById('tab-' + btn.dataset.tab).classList.add('active');
      searchInput.value = '';
      updateSearchClear();
      searchLock.classList.add('hidden');
      main.scrollTop = 0;
      document.querySelectorAll('main input:not([type=hidden]):not(#setting-miner-address), main textarea, main select:not(#setting-theme):not(#setting-loglevel)').forEach(el => {
        if (el.tagName === 'SELECT') el.selectedIndex = 0;
        else el.value = '';
      });
      try {
        if (btn.dataset.tab === 'log') refreshLog();
        if (btn.dataset.tab === 'wallet') await loadWalletInfo();
        if (btn.dataset.tab === 'names') await loadMyNames();
        if (btn.dataset.tab === 'auctions') await loadAuctions();
        if (btn.dataset.tab === 'transactions') await loadAllTransactions(true);
        if (btn.dataset.tab === 'settings') {
          try {
            const s = await __invoke('get_settings');
            document.getElementById('setting-miner-address').value = s.minerAddress || '';
            updateMiningToggleUI(s.miningEnabled);
          } catch(e) {}
          try {
            var proxyStatus = await __invoke('get_proxy_status');
            updateSetupChecks(proxyStatus);
          } catch(e) {}
          try {
            var mi = await rpc('getmininginfo', [], { wallet: null });
            if (mi.result) {
              var cpus = mi.result.cpus || 1;
              var maxThreads = Math.max(1, cpus - 1);
              var current = mi.result.minerThreads || maxThreads;
              var slider = document.getElementById('setting-miner-threads');
              slider.min = 1;
              slider.max = maxThreads;
              slider.value = current;
              updateSliderFill(slider);
              document.getElementById('miner-thread-label').textContent = current + ' / ' + cpus;
            }
          } catch(e) {}
        }
      } catch(e) {}
      main.style.opacity = '1';
    });
  });

  var _fbcIconHTML = '<span class="fbc-icon"></span>';

  // Populate `el` with a fistbump icon + formatted amount using DOM nodes
  // (no innerHTML). Replaces the common `el.innerHTML = formatFBC(bumps)`
  // pattern so we don't touch HTML interpretation when the wrapping code
  // is near user-controlled input.
  function setFbcAmount(el, bumps) {
    while (el.firstChild) el.removeChild(el.firstChild);
    var icon = document.createElement('span');
    icon.className = 'fbc-icon';
    el.appendChild(icon);
    var num = document.createElement('span');
    num.textContent = formatFBC(bumps, { noIcon: true });
    el.appendChild(num);
  }

  function formatFBC(bumps, opts) {
    if (bumps == null) return '0.00';
    var negative = bumps < 0;
    var abs = Math.abs(bumps);
    var num = (abs / 1000000).toLocaleString(undefined, {
      minimumFractionDigits: 2,
      maximumFractionDigits: 6
    });
    if (opts && opts.noIcon) return (negative ? '-' : '') + num;
    return (negative ? '-' : '') + _fbcIconHTML + num;
  }

  function timeAgo(ts) {
    const s = Math.floor(Date.now() / 1000) - ts;
    if (s < 60) return 'just now';
    if (s < 3600) return Math.floor(s / 60) + 'm ago';
    if (s < 86400) return Math.floor(s / 3600) + 'h ago';
    if (s < 2592000) return Math.floor(s / 86400) + 'd ago';
    return new Date(ts * 1000).toLocaleDateString();
  }

  var _protocolParams = null;
  async function getProtocolParams() {
    if (!_protocolParams) {
      var r = await rpc('getprotocolparams', [], { wallet: null });
      if (r.result) _protocolParams = r.result;
    }
    return _protocolParams || {};
  }

  function blocksToTime(blocks) {
    var secs = blocks * 120; // ~2 min per block
    if (secs < 3600) return Math.ceil(secs / 60) + 'm';
    if (secs < 86400) return Math.round(secs / 3600) + 'h';
    return Math.round(secs / 86400) + 'd';
  }

  const stateColors = {
    'PENDING': 'purple', 'OPENING': 'blue', 'BIDDING': 'yellow', 'REVEAL': 'yellow',
    'CLOSED': 'gray', 'EXPIRED': 'red', 'TRANSFER': 'purple', 'REVOKED': 'purple',
    'AVAILABLE': 'green', 'UNAVAILABLE': 'red', 'UPCOMING': 'yellow',
  };
  function stateBadge(state, restriction, rolloutHeight, parentIssue) {
    var display = state;
    if (state === 'INACTIVE') {
      if (restriction === 'blacklisted' || parentIssue) {
        display = 'UNAVAILABLE';
      } else if (rolloutHeight && rolloutHeight > _esLastHeight) {
        display = 'UPCOMING';
      } else {
        display = 'AVAILABLE';
      }
    }
    const color = stateColors[display] || 'gray';
    return '<span class="badge badge-' + color + '">' + esc(display) + '</span>';
  }

  // ---- Overview ----

  async function refresh() {
    const dot = document.getElementById('status-dot');
    const label = document.getElementById('sync-status');
    try {
      const info = await rpc('getblockchaininfo');
      if (info.error) throw new Error(info.error);
      const r = info.result;
      const synced = !r.verificationprogress || r.verificationprogress >= 0.999 || r.blocks === r.headers;

      document.getElementById('about-network').textContent = r.chain;
      document.getElementById('about-height').innerHTML = '<a href="' + EXPLORER + '/block/' + r.blocks + '" target="_blank">' + r.blocks.toLocaleString() + '</a>';
      if (r.version) document.getElementById('about-node').textContent = 'fbd ' + r.version;

      var peerCount = 0;
      try {
        const peers = await rpc('getpeerinfo');
        if (peers.result && Array.isArray(peers.result)) {
          peerCount = peers.result.length;
          document.getElementById('about-peers').textContent = peerCount;
        }
      } catch(e) {}

      // Always update status bar from RPC data; SSE events will overwrite in real time
      _esLastHeight = r.blocks;
      _esLastProgress = r.verificationprogress;
      _esPeerCount = peerCount;
      if (peerCount === 0) {
        dot.className = 'status-dot error';
        label.textContent = 'No peers';
      } else if (synced) {
        dot.className = 'status-dot connected';
        label.textContent = 'Block ' + r.blocks.toLocaleString();
      } else {
        dot.className = 'status-dot syncing';
        label.textContent = 'Syncing ' + (r.verificationprogress * 100).toFixed(1) + '%';
      }

      try {
        const bal = await rpc('getbalance');
        if (bal.result && typeof bal.result === 'object') {
          updateBalance(bal.result);
        }
      } catch(e) {}

      // Keep encryption/lock state in sync
      try {
        var wi = await rpc('getwalletinfo');
        if (wi.result) {
          walletEncrypted = !!wi.result.encrypted;
          walletUnlocked = !wi.result.encrypted || !!wi.result.unlocked;
          updateLockIndicator();
        }
      } catch(e) {}

      refreshTxList();
      if (synced) refreshOverviewActions();

      // Refresh whichever tab is active
      var activeTab = document.querySelector('.page.active');
      if (activeTab) {
        var id = activeTab.id;
        if (id === 'tab-names') loadMyNames();
        else if (id === 'tab-auctions') loadAuctions();
        else if (id === 'tab-wallet') loadWalletInfo();
      }
    } catch(e) {
      dot.className = 'status-dot connecting';
      label.textContent = 'Connecting...';
    }
  }

  async function refreshTxList() {
    try {
      var res = await rpc('listtransactions', [10]);
      var el = document.getElementById('tx-list');
      if (res.error) return;
      if (!res.result || !Array.isArray(res.result) || res.result.length === 0) {
        el.className = 'empty-state';
        el.textContent = 'No transactions yet';
        return;
      }
      var txs = res.result;
      el.className = '';
      el.innerHTML = txs.map(function(tx) {
        return renderTxItem(tx);
      }).join('');
    } catch(e) {}
  }

  // ---- All Transactions ----

  var allTxOffset = 0;
  var allTxPageSize = 20;

  function hexToName(hex) {
    if (!hex) return '';
    try {
      var b = [];
      for (var j = 0; j < hex.length; j += 2) b.push(parseInt(hex.substr(j, 2), 16));
      return String.fromCharCode.apply(null, b);
    } catch(e) { return ''; }
  }

  function renderTxItem(tx) {
    var hash = tx.txid || '';
    var time = tx.timestamp ? timeAgo(tx.timestamp) : '';
    var amt = tx.net || 0;
    var sign = amt >= 0 ? 'positive' : 'negative';
    var prefix = amt >= 0 ? '+' : '-';

    // Covenants from RPC already include names (e.g., "OPEN igloo")
    var covParts = [];
    if (tx.coinbase) {
      covParts.push('COINBASE');
    }
    var covs = (tx.covenants || []).filter(function(c) { return c !== 'NONE'; });
    covs.forEach(function(c) {
      var parts = c.split(' ');
      var label = parts[0].toUpperCase() + (parts.length > 1 ? ' ' + parts.slice(1).join(' ') : '');
      if (covParts.indexOf(label) === -1) covParts.push(label);
    });
    if (covParts.length === 0) {
      if (tx.type === 'send') covParts.push('SEND');
      else if (tx.type === 'receive') covParts.push('RECEIVE');
      else if (tx.type === 'self') covParts.push('SELF');
    }

    // Blue=auction, Teal=name gained, Yellow=maintenance, Purple=name loss, Red=coins out, Green=coins in, Gray=neutral
    var covColors = { OPEN:'blue', BID:'blue', REVEAL:'blue', REGISTER:'teal', REDEEM:'green', UPDATE:'yellow', RENEW:'yellow', TRANSFER:'purple', FINALIZE:'purple', REVOKE:'purple', COINBASE:'green', RECEIVE:'green', SEND:'red', SELF:'gray' };
    var covHTML = covParts.map(function(c) {
      var parts = c.split(' ');
      var color = covColors[parts[0]] || 'blue';
      var action = '<span class="badge badge-' + color + '">' + esc(parts[0]) + '</span>';
      var nameTxt = parts.length > 1 ? parts.slice(1).join(' ') : '';
      var name = nameTxt ? ' <a href="#" class="tx-name name-link" data-name="' + esc(nameTxt) + '">' + esc(nameTxt) + '</a>' : '';
      return action + name;
    }).join(' ');

    return '<div class="tx-item">' +
      '<div class="tx-left">' +
        '<div class="tx-covs">' + covHTML + '</div>' +
        '<a href="' + EXPLORER + '/tx/' + esc(hash) + '" target="_blank" class="tx-hash tx-link mono-sm">' + esc(hash) + '</a>' +
      '</div>' +
      '<div class="tx-right">' +
        '<span class="tx-amount ' + sign + '">' + prefix + formatFBC(Math.abs(amt)) + '</span>' +
        (time ? '<span class="tx-time">' + esc(time) + '</span>' : '') +
      '</div>' +
    '</div>';
  }

  async function loadAllTransactions(reset) {
    if (reset) allTxOffset = 0;
    try {
      var res = await rpc('listtransactions', [allTxPageSize, allTxOffset]);
      var el = document.getElementById('all-tx-list');
      var moreBtn = document.getElementById('tx-load-more');
      var txs = res.result || [];

      if (allTxOffset === 0 && txs.length === 0) {
        el.className = 'empty-state';
        el.textContent = 'No transactions yet';
        moreBtn.classList.add('hidden');
        return;
      }

      var html = txs.map(function(tx) {
        return renderTxItem(tx);
      }).join('');

      if (allTxOffset === 0) {
        el.className = '';
        el.innerHTML = html;
      } else {
        el.innerHTML += html;
      }

      allTxOffset += txs.length;
      if (txs.length >= allTxPageSize) {
        moreBtn.classList.remove('hidden');
      } else {
        moreBtn.classList.add('hidden');
      }
    } catch(e) {
      document.getElementById('all-tx-list').textContent = 'Error loading transactions';
    }
  }

  document.getElementById('btn-tx-load-more').addEventListener('click', function() {
    loadAllTransactions(false);
  });

  async function refreshOverviewActions() {
    var el = document.getElementById('overview-actions');
    try {
      var [actionsRes, chainRes] = await Promise.all([
        rpc('getwalletactions'),
        rpc('getblockchaininfo', [], { wallet: null }),
      ]);
      var actions = actionsRes.result || {};
      var needsReveal = actions.reveal || [];
      var needsRegister = actions.register || [];
      var needsRedeem = actions.redeem || [];
      var needsRenewal = actions.renew || [];
      var needsFinalize = actions.finalize || [];
      var needsRepair = actions.repair || [];

      var revealCount = needsReveal.length;
      var registerCount = needsRegister.length;
      var renewalCount = needsRenewal.length;

      var cards = '';
      function nameLinks(names) {
        return names.map(function(n) { return '<a href="#" class="name-link tx-name" data-name="' + esc(n) + '">' + esc(n) + '</a>'; }).join(', ');
      }

      if (needsRepair.length > 0) {
        var repairList = needsRepair.map(function(a) { return a.name; }).filter(Boolean);
        cards += '<div class="overview-action-card">' +
          '<div class="overview-action-icon"><svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M8 1v6M8 11v1"/><circle cx="8" cy="8" r="7"/></svg></div>' +
          '<div class="overview-action-info">' +
            '<div class="overview-action-title">' + repairList.length + ' bid' + (repairList.length > 1 ? 's' : '') + (repairList.length > 1 ? ' need' : ' needs') + ' repair</div>' +
            '<div class="overview-action-desc">' + nameLinks(repairList) + '</div>' +
          '</div>' +
          '<div class="btn btn-sm overview-repair-action" data-names=\'' + JSON.stringify(needsRepair.map(function(a) { return { name: a.name, nameHash: a.nameHash, lockup: a.lockup }; })) + '\'>Repair</div>' +
        '</div>';
      }
      if (revealCount > 0) {
        var revealNames = [...new Set(needsReveal.map(function(a) { return a.name; }).filter(Boolean))];
        cards += '<div class="overview-action-card">' +
          '<div class="overview-action-icon"><svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M2 8a6 6 0 0 1 12 0"/><circle cx="8" cy="8" r="2"/></svg></div>' +
          '<div class="overview-action-info">' +
            '<div class="overview-action-title">' + revealCount + ' bid' + (revealCount > 1 ? 's' : '') + (revealCount > 1 ? ' need' : ' needs') + ' revealing</div>' +
            '<div class="overview-action-desc">' + nameLinks(revealNames) + '</div>' +
          '</div>' +
          '<div class="btn primary btn-sm overview-batch-action" data-action="reveal">Reveal</div>' +
        '</div>';
      }
      if (registerCount > 0) {
        var registerNames = needsRegister.map(function(a) { return a.name; }).filter(Boolean);
        cards += '<div class="overview-action-card">' +
          '<div class="overview-action-icon"><svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M4 8l3 3 5-6"/></svg></div>' +
          '<div class="overview-action-info">' +
            '<div class="overview-action-title">' + registerCount + ' name' + (registerCount > 1 ? 's' : '') + ' ready to register</div>' +
            '<div class="overview-action-desc">' + nameLinks(registerNames) + '</div>' +
          '</div>' +
          '<div class="btn primary btn-sm overview-batch-action" data-action="register">Register</div>' +
        '</div>';
      }
      if (needsRedeem.length > 0) {
        var redeemList = [...new Set(needsRedeem.map(function(a) { return a.name; }).filter(Boolean))];
        cards += '<div class="overview-action-card">' +
          '<div class="overview-action-icon"><svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M12 4L4 12M4 4l8 8"/></svg></div>' +
          '<div class="overview-action-info">' +
            '<div class="overview-action-title">' + redeemList.length + ' losing bid' + (redeemList.length > 1 ? 's' : '') + ' to redeem</div>' +
            '<div class="overview-action-desc">' + nameLinks(redeemList) + '</div>' +
          '</div>' +
          '<div class="btn primary btn-sm overview-batch-action" data-action="redeem">Redeem</div>' +
        '</div>';
      }
      if (needsFinalize.length > 0) {
        var finalizeNames = needsFinalize.map(function(a) { return a.name; }).filter(Boolean);
        cards += '<div class="overview-action-card">' +
          '<div class="overview-action-icon"><svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M2 8h12M9 4l4 4-4 4"/></svg></div>' +
          '<div class="overview-action-info">' +
            '<div class="overview-action-title">' + finalizeNames.length + ' name' + (finalizeNames.length > 1 ? 's' : '') + ' ready to finalize</div>' +
            '<div class="overview-action-desc">' + nameLinks(finalizeNames) + '</div>' +
          '</div>' +
          '<div class="btn primary btn-sm overview-batch-action" data-action="finalize-transfer">Finalize</div>' +
        '</div>';
      }
      if (renewalCount > 0) {
        var renewalNames = needsRenewal.map(function(a) { return a.name; }).filter(Boolean);
        cards += '<div class="overview-action-card">' +
          '<div class="overview-action-icon"><svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M1 8a7 7 0 1 1 14 0A7 7 0 0 1 1 8z"/><path d="M8 4v4l2.5 2.5"/></svg></div>' +
          '<div class="overview-action-info">' +
            '<div class="overview-action-title">' + renewalCount + ' name' + (renewalCount > 1 ? 's' : '') + ' expiring soon</div>' +
            '<div class="overview-action-desc">' + nameLinks(renewalNames) + '</div>' +
          '</div>' +
          '<div class="btn primary btn-sm overview-batch-action" data-action="renew">Renew</div>' +
        '</div>';
      }

      if (cards) {
        el.innerHTML = '<div class="section-card"><div class="section-label">Needs Attention</div>' +
          cards + '<div id="overview-batch-status"></div></div>';
        el.classList.remove('hidden');
      } else {
        el.classList.add('hidden');
      }
    } catch(e) {
      el.classList.add('hidden');
    }
  }

  document.getElementById('overview-actions').addEventListener('click', async function(e) {
    var btn = e.target.closest('.overview-batch-action');
    if (!btn) return;
    var action = btn.dataset.action;
    var labels = {
      reveal: 'Reveal all pending bids',
      register: 'Register all won names',
      finalize: 'Register won names and redeem losing bids',
      redeem: 'Redeem all losing bids',
      'finalize-transfer': 'Finalize all pending transfers',
      renew: 'Renew all expiring names',
    };
    if (!await showConfirm((labels[action] || action) + '?')) return;
    btn.style.opacity = '0.5';
    btn.style.pointerEvents = 'none';
    var st = document.getElementById('overview-batch-status');
    if (st) st.innerHTML = '<div class="modal-loading">Processing...</div>';
    try {
      var manyArg = action === 'finalize' ? 'register, redeem' : action === 'finalize-transfer' ? 'finalize' : action;
      var r = await rpc('sendmany', [manyArg]);
      if (r.error) throw new Error(r.error);
      if (st) st.innerHTML = '';
      var txid = r.result && (r.result.txid || (r.result.txids && r.result.txids[0]));
      showToast('Transaction broadcast', txid || null);
      btn.textContent = 'Done';
      btn.className = 'btn btn-sm';
      setTimeout(function() { refreshOverviewActions(); }, 2000);
    } catch(e2) {
      if (st) st.innerHTML = '<div class="error-msg">' + esc(e2.message) + '</div>';
      btn.style.opacity = '';
      btn.style.pointerEvents = '';
    }
  });

  document.getElementById('overview-actions').addEventListener('click', async function(e) {
    var btn = e.target.closest('.overview-repair-action');
    if (!btn) return;
    var bidsData = JSON.parse(btn.dataset.names || '[]');
    if (!bidsData.length) return;

    var bid = bidsData[0];
    var label = bid.name || '(unknown)';
    document.getElementById('repair-name').textContent = label;
    document.getElementById('repair-lockup').innerHTML = 'Lockup: ' + formatFBC(bid.lockup);
    document.getElementById('repair-status').textContent = '';
    openModal('repair-modal');

    var submitBtn = document.getElementById('repair-submit');
    var autoBtn = document.getElementById('repair-auto');
    var newSubmit = submitBtn.cloneNode(true);
    var newAuto = autoBtn.cloneNode(true);
    submitBtn.replaceWith(newSubmit);
    autoBtn.replaceWith(newAuto);

    newSubmit.addEventListener('click', async function() {
      var val = document.getElementById('repair-value').value;
      if (!val) return;
      var rs = document.getElementById('repair-status');
      rs.innerHTML = '<div class="modal-loading">Recovering...</div>';
      try {
        var r = await rpc('repairbid', [bid.name || '', parseFloat(val)]);
        if (r.error) throw new Error(r.error);
        rs.innerHTML = '<div class="success-msg">Bid recovered!</div>';
        setTimeout(function() { closeModal('repair-modal'); refreshOverviewActions(); }, 1500);
      } catch(e) { rs.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>'; }
    });
    newAuto.addEventListener('click', async function() {
      var rs = document.getElementById('repair-status');
      rs.innerHTML = '<div class="modal-loading">Auto-recovering...</div>';
      try {
        var r = await rpc('repairbid', [bid.name || '']);
        if (r.error) throw new Error(r.error);
        var recoveredFBC = r.result && r.result.value ? formatFBC(r.result.value) : '';
        rs.innerHTML = '<div class="success-msg">Bid recovered!' + (recoveredFBC ? ' (' + recoveredFBC + ')' : '') + '</div>';
        setTimeout(function() { closeModal('repair-modal'); refreshOverviewActions(); }, 1500);
      } catch(e) { rs.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>'; }
    });
  });

  document.getElementById('overview-actions').addEventListener('click', function(e) {
    var link = e.target.closest('.name-link');
    if (!link) return;
    e.preventDefault();
    openNameDetail(link.dataset.name);
  });

  // ---- Names ----

  async function loadMyNames() {
    try {
      var [res, params] = await Promise.all([rpc('getnames'), getProtocolParams()]);
      var renewalWindow = params.renewalWindow || 262800;
      const el = document.getElementById('my-names');
      const names = res.result || [];
      const auctionTypes = ['OPEN', 'BID', 'REVEAL', 'REDEEM', 'NONE'];
      const registered = names.filter(n => n.name.state === 'CLOSED' && !auctionTypes.includes(n.covenantType))
        .sort((a, b) => (a.name.string || '').localeCompare(b.name.string || ''));
      if (registered.length === 0) {
        el.className = 'empty-state';
        el.textContent = 'No registered names';
        return;
      }
      el.className = '';
      el.innerHTML = '<table class="names-table"><tr><th>Name</th><th>Expires</th></tr>' +
        registered.map(n => {
          var expiry = n.name.renewal ? blockLink(n.name.renewal + renewalWindow) : '--';
          return '<tr>' +
            '<td>' + nameLink(n.name.string, n.name.hash) + '</td>' +
            '<td>' + expiry + '</td>' +
          '</tr>';
        }).join('') +
        '</table>';
    } catch(e) {
      document.getElementById('my-names').textContent = friendlyError(e.message) || 'Error loading names';
    }
  }

  // ---- Send / Receive modals ----

  function openModal(id) {
    var modal = document.getElementById(id);
    modal.querySelectorAll('input:not([type=hidden]), textarea').forEach(function(el) { el.value = ''; });
    modal.querySelectorAll('.error-msg, .success-msg').forEach(function(el) { el.remove(); });
    modal.classList.remove('hidden');
    var firstInput = modal.querySelector('input:not([type=hidden]), textarea');
    if (firstInput) setTimeout(function() { firstInput.focus(); }, 50);
  }
  function closeModal(id) { document.getElementById(id).classList.add('hidden'); }

  // Global keyboard shortcuts for modals
  document.addEventListener('keydown', function(e) {
    var openModal = document.querySelector('.modal-overlay:not(.hidden)');
    if (!openModal) return;
    // confirm-modal and unlock-modal are handled by their own listeners
    var isConfirm = openModal.id === 'confirm-modal' || openModal.id === 'unlock-modal';
    if (e.key === 'Tab') {
      var focusable = openModal.querySelectorAll('input:not([type=hidden]), textarea, button, [tabindex]');
      if (focusable.length === 0) { e.preventDefault(); return; }
      var first = focusable[0], last = focusable[focusable.length - 1];
      if (e.shiftKey) {
        if (document.activeElement === first || !openModal.contains(document.activeElement)) { e.preventDefault(); last.focus(); }
      } else {
        if (document.activeElement === last || !openModal.contains(document.activeElement)) { e.preventDefault(); first.focus(); }
      }
      return;
    }
    if (e.key === 'Escape' && !isConfirm) {
      var sendModal = document.getElementById('send-modal');
      if (!sendModal.classList.contains('hidden')) { closeSendModal(); return; }
      var recvModal = document.getElementById('receive-modal');
      if (!recvModal.classList.contains('hidden')) { closeModal('receive-modal'); return; }
      var cosignModal = document.getElementById('cosign-modal');
      if (cosignModal && !cosignModal.classList.contains('hidden')) { closeModal('cosign-modal'); return; }
      var msModal = document.getElementById('multisig-modal');
      if (msModal && !msModal.classList.contains('hidden')) { closeModal('multisig-modal'); return; }
      var qrShowModal = document.getElementById('qr-show-modal');
      if (!qrShowModal.classList.contains('hidden')) { closeModal('qr-show-modal'); return; }
      var qrScanModal = document.getElementById('qr-scan-modal');
      if (!qrScanModal.classList.contains('hidden')) { stopScanStream(); closeModal('qr-scan-modal'); return; }
    }
    if (e.key === 'Enter' && !isConfirm) {
      e.preventDefault();
      var btns = openModal.querySelectorAll('.btn.primary');
      for (var i = 0; i < btns.length; i++) {
        if (btns[i].offsetParent !== null && btns[i].style.pointerEvents !== 'none') { btns[i].click(); return; }
      }
    }
  }, true);

  document.getElementById('btn-show-send').addEventListener('click', () => openModal('send-modal'));
  function closeSendModal() {
    pendingPstx = null;
    setSendMaxMode(false);
    document.getElementById('send-confirm').classList.add('hidden');
    document.getElementById('send-form').classList.remove('hidden');
    document.getElementById('send-status').innerHTML = '';
    document.getElementById('send-confirm-status').innerHTML = '';
    var sendBtn = document.getElementById('send-do');
    sendBtn.style.opacity = '';
    sendBtn.style.pointerEvents = '';
    closeModal('send-modal');
  }
  document.getElementById('send-modal-close').addEventListener('click', closeSendModal);
  document.getElementById('send-modal').addEventListener('click', (e) => {
    if (e.target === e.currentTarget) closeSendModal();
  });

  document.getElementById('btn-show-receive').addEventListener('click', () => {
    openModal('receive-modal');
    loadReceiveAddress();
  });
  document.getElementById('receive-modal-close').addEventListener('click', () => closeModal('receive-modal'));
  document.getElementById('receive-modal').addEventListener('click', (e) => {
    if (e.target === e.currentTarget) closeModal('receive-modal');
  });

  // ---- Cosign / Broadcast modal ----

  function cosignUpdateButtons(sigCount, m) {
    var signBtn = document.getElementById('cosign-sign');
    var broadcastBtn = document.getElementById('cosign-broadcast');
    var statusEl = document.getElementById('cosign-status');
    if (sigCount >= m) {
      signBtn.classList.add('hidden');
      broadcastBtn.classList.remove('hidden');
      statusEl.innerHTML = '<div class="success-msg">Fully signed (' + sigCount + '/' + m + '). Ready to broadcast.</div>';
    } else {
      signBtn.classList.remove('hidden');
      broadcastBtn.classList.add('hidden');
      statusEl.innerHTML = '<div class="muted text-base">Signatures: ' + sigCount + ' of ' + m + '. Needs signing.</div>';
    }
  }

  var cosignCheckTimer = null;
  function cosignCheckInput() {
    clearTimeout(cosignCheckTimer);
    cosignCheckTimer = setTimeout(async () => {
      var hex = document.getElementById('cosign-input').value.trim();
      var signBtn = document.getElementById('cosign-sign');
      var broadcastBtn = document.getElementById('cosign-broadcast');
      var statusEl = document.getElementById('cosign-status');
      document.getElementById('cosign-result-section').classList.add('hidden');
      if (!hex) {
        signBtn.classList.remove('hidden');
        broadcastBtn.classList.add('hidden');
        statusEl.innerHTML = '';
        return;
      }
      try {
        var r = await rpc('decoderawtransaction', [hex], { wallet: null });
        if (r.error) throw new Error(r.error);
        // Check signature counts across all inputs
        var inputs = r.result.inputs || [];
        var minSigs = Infinity;
        var m = walletMultisigM;
        for (var i = 0; i < inputs.length; i++) {
          var inp = inputs[i];
          if (inp.signatures) {
            var parts = inp.signatures.split('/');
            var have = parseInt(parts[0]) || 0;
            if (have < minSigs) minSigs = have;
          }
          if (inp.multisig) {
            var msParts = inp.multisig.split('-of-');
            m = parseInt(msParts[0]) || m;
          }
        }
        if (minSigs === Infinity) minSigs = 0;
        cosignUpdateButtons(minSigs, m);
      } catch(e) {
        signBtn.classList.remove('hidden');
        broadcastBtn.classList.add('hidden');
        statusEl.innerHTML = '<div class="error-msg">Invalid transaction hex.</div>';
      }
    }, 300);
  }

  document.getElementById('btn-show-cosign').addEventListener('click', () => {
    document.getElementById('cosign-input').value = '';
    document.getElementById('cosign-output').value = '';
    document.getElementById('cosign-result-section').classList.add('hidden');
    document.getElementById('cosign-status').innerHTML = '';
    document.getElementById('cosign-sign').classList.remove('hidden');
    document.getElementById('cosign-broadcast').classList.add('hidden');
    openModal('cosign-modal');
  });
  document.getElementById('cosign-close').addEventListener('click', () => closeModal('cosign-modal'));
  document.getElementById('cosign-modal').addEventListener('click', (e) => {
    if (e.target === e.currentTarget) closeModal('cosign-modal');
  });

  document.getElementById('cosign-input').addEventListener('input', cosignCheckInput);

  document.getElementById('cosign-sign').addEventListener('click', async () => {
    var hex = document.getElementById('cosign-input').value.trim();
    var statusEl = document.getElementById('cosign-status');
    if (!hex) { statusEl.innerHTML = '<div class="error-msg">Paste a PSTX hex to sign.</div>'; return; }
    statusEl.innerHTML = '<div class="modal-loading">Signing...</div>';
    try {
      var ok = await requireUnlock();
      if (!ok) { statusEl.innerHTML = ''; return; }
      var r = await rpc('signtx', [hex]);
      if (r.error) throw new Error(r.error);
      var signedHex = r.result.pstx || r.result.hex || r.result;
      var sigCount = r.result.signatures || 1;
      document.getElementById('cosign-output').value = signedHex;
      document.getElementById('cosign-result-section').classList.remove('hidden');
      if (sigCount >= walletMultisigM) {
        document.getElementById('cosign-broadcast-signed').classList.remove('hidden');
        statusEl.innerHTML = '<div class="success-msg">Signed (' + sigCount + '/' + walletMultisigM + '). Fully signed \u2014 ready to broadcast.</div>';
      } else {
        document.getElementById('cosign-broadcast-signed').classList.add('hidden');
        statusEl.innerHTML = '<div class="success-msg">Signed (' + sigCount + '/' + walletMultisigM + '). Copy and send back to the initiator.</div>';
      }
    } catch(e) {
      statusEl.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  });

  document.getElementById('cosign-copy').addEventListener('click', () => {
    var hex = document.getElementById('cosign-output').value;
    if (!hex) return;
    navigator.clipboard.writeText(hex).then(() => showToast('Copied to clipboard'));
  });

  async function doCosignBroadcast(hex) {
    var statusEl = document.getElementById('cosign-status');
    if (!hex) { statusEl.innerHTML = '<div class="error-msg">No transaction to broadcast.</div>'; return; }
    statusEl.innerHTML = '<div class="modal-loading">Broadcasting...</div>';
    try {
      var r = await rpc('broadcasttx', [hex], { wallet: null });
      if (r.error) throw new Error(r.error);
      var txid = r.result.txid || r.result;
      closeModal('cosign-modal');
      showToast('Transaction broadcast', typeof txid === 'string' ? txid : null);
    } catch(e) {
      statusEl.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  }

  document.getElementById('cosign-broadcast').addEventListener('click', () => {
    doCosignBroadcast(document.getElementById('cosign-input').value.trim());
  });

  document.getElementById('cosign-broadcast-signed').addEventListener('click', () => {
    doCosignBroadcast(document.getElementById('cosign-output').value.trim());
  });

  let pendingPstx = null;
  // When true, createtx is called with subtractFee so the recipient receives
  // (entered amount - fee) and the wallet is debited exactly the entered amount.
  // Set by the Max button, cleared as soon as the user edits the amount.
  let sendMaxMode = false;

  // Resolve a name or address to a sendable address.
  // Returns { address, name } where name is the input if it was a name, null if direct address.
  async function resolveRecipient(input) {
    // Try as address first
    var valRes = await rpc('validateaddress', [input]);
    if (valRes.result && valRes.result.isvalid) {
      return { address: input, name: null };
    }
    // Fall back to server-side name resolution
    var resolveRes = await rpc('resolveaddress', [input], { wallet: null });
    if (resolveRes.error) throw new Error(resolveRes.error);
    return { address: resolveRes.result, name: input };
  }

  function showSendError(msg) {
    var el = document.getElementById('send-status');
    el.textContent = '';
    var div = document.createElement('div');
    div.className = 'error-msg';
    div.textContent = msg;
    el.appendChild(div);
  }

  function setSendMaxMode(on) {
    sendMaxMode = on;
    document.getElementById('send-max').classList.toggle('primary', on);
  }

  document.getElementById('send-max').addEventListener('click', async () => {
    // Toggle off: clear the amount and deactivate.
    if (sendMaxMode) {
      document.getElementById('send-amount').value = '';
      setSendMaxMode(false);
      document.getElementById('send-status').textContent = '';
      return;
    }
    // Toggle on: pull current spendable and fill the amount.
    var b = _lastBalance;
    if (!b) {
      try {
        var res = await rpc('getbalance');
        if (res.error) throw new Error(res.error);
        b = res.result;
      } catch(e) {
        showSendError('Could not fetch balance.');
        return;
      }
    }
    var spendable = b.spendable || 0;
    if (spendable <= 0) {
      showSendError('No spendable balance.');
      return;
    }
    // Exact integer → decimal string conversion (avoids float precision loss).
    var s = String(spendable);
    var neg = false;
    if (s.charAt(0) === '-') { neg = true; s = s.slice(1); }
    while (s.length < 7) s = '0' + s;
    var whole = s.slice(0, -6);
    var frac = s.slice(-6).replace(/0+$/, '');
    document.getElementById('send-amount').value =
      (neg ? '-' : '') + (frac.length ? whole + '.' + frac : whole);
    setSendMaxMode(true);
    document.getElementById('send-status').textContent = '';
  });

  document.getElementById('send-amount').addEventListener('input', () => {
    // Manual edit keeps the amount but exits max mode.
    if (sendMaxMode) setSendMaxMode(false);
  });

  document.getElementById('send-review').addEventListener('click', async () => {
    const input = document.getElementById('send-address').value.trim();
    const amountFBC = parseFloat(document.getElementById('send-amount').value);
    const el = document.getElementById('send-status');
    if (!input || isNaN(amountFBC) || amountFBC <= 0) {
      el.innerHTML = '<div class="error-msg">Enter a valid address or name and amount.</div>';
      return;
    }
    el.innerHTML = '<div class="modal-loading">Resolving recipient...</div>';
    try {
      var resolved = await resolveRecipient(input);
      el.innerHTML = '<div class="modal-loading">Building transaction...</div>';
      const createRes = await rpc('createtx', ['none', resolved.address, amountFBC, sendMaxMode]);
      if (createRes.error) throw new Error(createRes.error);
      pendingPstx = createRes.result.pstx;

      const decodeRes = await rpc('decoderawtransaction', [pendingPstx]);
      if (decodeRes.error) throw new Error(decodeRes.error);
      const fee = decodeRes.result.fee || 0;
      const enteredDoo = Math.round(amountFBC * 1000000);
      // In max mode: recipient gets entered - fee, wallet is debited entered.
      // Normal mode: recipient gets entered, wallet is debited entered + fee.
      const recipientDoo = sendMaxMode ? enteredDoo - fee : enteredDoo;
      const totalDoo = sendMaxMode ? enteredDoo : enteredDoo + fee;

      var nameEl = document.getElementById('confirm-to-name');
      var addrEl = document.getElementById('confirm-to-address');
      // The .stack variant of .review-row-value hides the first <span>
      // when it's :empty, so we can just unconditionally set both text
      // contents instead of toggling display.
      nameEl.textContent = resolved.name || '';
      addrEl.textContent = resolved.address;
      setFbcAmount(document.getElementById('confirm-amount'), recipientDoo);
      setFbcAmount(document.getElementById('confirm-fee'), fee);
      setFbcAmount(document.getElementById('confirm-total'), totalDoo);
      document.getElementById('send-confirm-status').innerHTML = '';
      document.getElementById('send-form').classList.add('hidden');
      document.getElementById('send-confirm').classList.remove('hidden');
      el.innerHTML = '';
    } catch(e) {
      el.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  });

  document.getElementById('send-back').addEventListener('click', () => {
    pendingPstx = null;
    document.getElementById('send-confirm').classList.add('hidden');
    document.getElementById('send-form').classList.remove('hidden');
  });

  document.getElementById('send-do').addEventListener('click', async () => {
    if (!pendingPstx) return;
    if (!await requireUnlock()) return;
    var sendBtn = document.getElementById('send-do');
    sendBtn.style.opacity = '0.5';
    sendBtn.style.pointerEvents = 'none';
    const el = document.getElementById('send-confirm-status');
    el.innerHTML = '<div class="modal-loading">Signing and broadcasting...</div>';
    try {
      const signRes = await rpc('signtx', [pendingPstx]);
      if (signRes.error) throw new Error(signRes.error);
      if (walletIsMultisig) {
        pendingPstx = null;
        closeSendModal();
        showMultisigFlow(signRes.result.pstx, signRes.result.signatures || 1);
      } else {
        const broadRes = await rpc('broadcasttx', [signRes.result.pstx]);
        if (broadRes.error) throw new Error(broadRes.error);
        const txid = broadRes.result.txid;
        pendingPstx = null;
        closeSendModal();
        showToast('Transaction sent', txid);
        refresh();
      }
    } catch(e) {
      el.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
      sendBtn.style.opacity = '';
      sendBtn.style.pointerEvents = '';
    }
  });

  // ---- Receive ----

  let currentAddress = null;

  async function loadReceiveAddress() {
    const status = document.getElementById('recv-status');
    const addrEl = document.getElementById('recv-addr');
    const actionsEl = document.getElementById('recv-actions');
    if (currentAddress) {
      addrEl.textContent = currentAddress;
      addrEl.classList.remove('hidden');
      actionsEl.classList.remove('hidden');
      status.classList.add('hidden');
      return;
    }
    status.innerHTML = '<div class="modal-loading">Generating address...</div>';
    status.classList.remove('hidden');
    addrEl.classList.add('hidden');
    actionsEl.classList.add('hidden');
    try {
      const res = await rpc('getnewaddress');
      if (res.error) throw new Error(res.error);
      currentAddress = res.result;
      addrEl.textContent = currentAddress;
      addrEl.classList.remove('hidden');
      actionsEl.classList.remove('hidden');
      status.classList.add('hidden');
    } catch(e) {
      status.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  }

  document.getElementById('copy-addr').addEventListener('click', () => {
    if (!currentAddress) return;
    navigator.clipboard.writeText(currentAddress);
    const b = document.getElementById('copy-addr');
    b.textContent = 'Copied';
    setTimeout(() => { b.textContent = 'Copy Address'; }, 1500);
  });

  document.getElementById('recv-new').addEventListener('click', () => {
    currentAddress = null;
    loadReceiveAddress();
  });

  // ---- Global Name Search ----

  var searchInput = document.getElementById('global-name-search');
  var searchClear = document.getElementById('search-clear');

  function updateSearchClear() {
    searchClear.classList.toggle('hidden', !searchInput.value);
    if (!searchInput.value) searchLock.classList.add('hidden');
  }

  searchInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      const val = e.target.value.trim().toLowerCase().replace(/\.+$/, '');
      if (!val) return;
      searchInput.value = val;
      searchInput.blur();
      openNameDetail(val);
    }
    if (e.key === 'Escape' && searchInput.value) {
      e.preventDefault();
      searchInput.value = '';
      updateSearchClear();
    }
  });

  searchInput.addEventListener('input', function() {
    var v = searchInput.value.toLowerCase().replace(/[^a-z0-9.\-]/g, '');
    v = v.replace(/\.{2,}/g, '.');   // collapse sequential dots
    v = v.replace(/^[.\-]+/, '');     // no leading dot or hyphen
    // no hyphen at start/end of a label (after each dot)
    v = v.replace(/\.-/g, '.').replace(/-\./g, '.');
    if (v !== searchInput.value) searchInput.value = v;
    updateSearchClear();
  });

  searchClear.addEventListener('click', () => {
    searchInput.value = '';
    searchClear.classList.add('hidden');
    searchLock.classList.add('hidden');
    searchInput.focus();
  });


  document.addEventListener('click', (e) => {
    const link = e.target.closest('.name-link');
    if (link) {
      e.preventDefault();
      openNameDetail(link.dataset.name);
      return;
    }
    const explorerLink = e.target.closest('.name-explorer-link');
    if (explorerLink) {
      e.preventDefault();
      var url = explorerLink.dataset.url;
      if (url) {
        showConfirm(url, { title: 'Open external link?', okText: 'Open' }).then(function(ok) {
          if (ok) __invoke('open_external', { url: url });
        });
      }
    }
  });

  // Pending name actions — shown as banners until next block confirms them
  // Keyed per wallet: { walletName: { name: { action, txid, ... } } }
  var allPendingNameActions = {};
  var currentNameDetail = null; // name currently being viewed

  function getPendingNameActions() {
    if (!activeWallet) return {};
    return allPendingNameActions[activeWallet] || (allPendingNameActions[activeWallet] = {});
  }

  function setPendingNameAction(name, action, txid, extra) {
    var pending = getPendingNameActions();
    pending[name] = Object.assign({ action: action, txid: txid }, extra || {});
  }

  async function openNameDetail(name) {
    var isRefresh = currentNameDetail === name && document.getElementById('tab-namedetail').classList.contains('active');
    currentNameDetail = name;
    var main = document.querySelector('main');
    var savedScroll = isRefresh ? main.scrollTop : 0;
    if (!isRefresh) {
      // Switch to name detail page with transition
      main.style.opacity = '0';
      await new Promise(r => setTimeout(r, 100));
      document.querySelectorAll('.nav-item').forEach(b => b.classList.remove('active'));
      document.querySelectorAll('.page').forEach(t => t.classList.remove('active'));
      document.getElementById('tab-namedetail').classList.add('active');
      main.scrollTop = 0;
    }
    document.getElementById('nd-title').innerHTML = esc(name) + ' <a href="#" class="name-explorer-link" data-url="' + EXPLORER + '/name/' + encodeURIComponent(name) + '">View on Explorer</a>';
    document.getElementById('nd-info').innerHTML = '<div class="name-result-card"><div class="modal-loading">Loading...</div></div>';
    document.getElementById('nd-bid-section').innerHTML = '';
    document.getElementById('nd-records').innerHTML = '';
    document.getElementById('nd-transfer-section').innerHTML = '';
    document.getElementById('nd-bids').innerHTML = '';
    main.style.opacity = '1';

    try {
      const [infoRes, bidsRes, auctionBidsRes] = await Promise.all([
        rpc('getnameinfo', [name]),
        rpc('getbids', [name]),
        rpc('getauctionbids', [name], { wallet: null }),
      ]);
      if (infoRes.error) throw new Error(infoRes.error);
      const info = infoRes.result;
      // Filter to current auction only — exclude bids from previous auction rounds
      const auctionStart = info.height || 0;
      const bids = (bidsRes.result || []).filter(function(b) { return b.height >= auctionStart || (b.height <= 0 && b.own); });
      const networkBids = (auctionBidsRes.result || []).filter(function(b) { return b.height >= auctionStart || (b.height === 0 && b.pending); });

      // Check ownership
      var isMine = false;
      if (info.owner && info.owner.address) {
        try {
          var ownerCheck = await rpc('validateaddress', [info.owner.address]);
          isMine = ownerCheck.result && ownerCheck.result.ismine;
        } catch(e) {}
      }

      // Check for OUR pending (unconfirmed) transactions on this name
      var walletPending = getPendingNameActions();
      var pending = walletPending[name] || null;
      if (pending && pending.txid) {
        // Verify it's still unconfirmed
        try {
          var confirmRes = await rpc('gettransaction', [pending.txid]);
          if (confirmRes.error) {
            // tx not in mempool anymore (confirmed or dropped) — clear pending
            delete walletPending[name];
            pending = null;
          } else if (confirmRes.result && confirmRes.result.confirmations > 0) {
            delete walletPending[name];
            pending = null;
          }
        } catch(e) { delete walletPending[name]; pending = null; }
      }
      if (pending) {
        if (pending.action === 'Transfer' || pending.action === 'TRANSFER') info.state = 'TRANSFER';
      }

      // Info card — stats grid matching explorer layout
      let restriction = info.restriction ? ' <span class="badge badge-red">' + esc(info.restriction) + '</span>' : '';
      var parentIssueShort = '';
      if (info.parentIssue) {
        if (info.parentIssue.includes('not allow')) parentIssueShort = 'subdomains disabled';
        else if (info.parentIssue.includes('not registered')) parentIssueShort = 'parent not registered';
        else if (info.parentIssue.includes('blacklisted')) parentIssueShort = 'parent blacklisted';
        else if (info.parentIssue.includes('ICANN')) parentIssueShort = 'ICANN reserved';
        else parentIssueShort = 'unavailable';
      }
      const parentIssueBadge = parentIssueShort ? ' <span class="badge badge-red">' + esc(parentIssueShort) + '</span>' : '';
      const dnssec = info.dnssecRequired;

      const isBlacklisted = info.restriction === 'blacklisted';
      const isPreReveal = ['OPENING', 'BIDDING', 'PENDING'].includes(info.state);
      var stats = [];
      stats.push({ label: 'State', value: stateBadge(info.state, info.restriction, info.rolloutHeight, info.parentIssue) + restriction + parentIssueBadge });

      if (info.state === 'INACTIVE') {
        stats.push({ label: 'Minimum Bid', value: info.minimumBid ? formatFBC(info.minimumBid) : '<span class="muted">-</span>' });
        stats.push({ label: 'Available', value: (info.rolloutHeight > 0 && !isBlacklisted) ? blockLink(info.rolloutHeight) : '<span class="muted">Now</span>' });
      } else if (isPreReveal) {
        var totalBids = networkBids.length > 0 ? networkBids.length : bids.length;
        var totalLockup = (networkBids.length > 0 ? networkBids : bids).reduce(function(sum, b) { return sum + (b.lockup || 0); }, 0);
        stats.push({ label: 'Bids', value: totalBids });
        stats.push({ label: 'Total Lockup', value: totalLockup ? formatFBC(totalLockup) : '<span class="muted">-</span>' });
      } else if (info.state === 'REVEAL') {
        var allBidsR = networkBids.length > 0 ? networkBids : bids;
        var revealedCount = allBidsR.filter(function(b) { return b.revealed || b.revealedValue != null; }).length;
        var totalBidsR = allBidsR.length;
        // If we have a pending reveal in mempool, count our unrevealed bids as about-to-be-revealed
        if (pending && pending.action === 'Reveal') {
          var ourUnrevealed = bids.filter(function(b) { return b.own && !b.revealed && !b.needsRepair; }).length;
          revealedCount = Math.min(revealedCount + ourUnrevealed, totalBidsR);
        }
        stats.push({ label: 'Reveals', value: revealedCount + ' / ' + totalBidsR });
        stats.push({ label: 'Highest Bid', value: formatFBC(info.highest) });
      } else if (info.state === 'CLOSED' && !info.registered) {
        stats.push({ label: 'Winning Bid', value: info.highest > 0 ? formatFBC(info.highest) : '<span class="muted">No bids</span>' });
        stats.push({ label: 'Register By', value: info.expirationHeight ? blockLink(info.expirationHeight) : '<span class="muted">-</span>' });
      } else if (info.registered) {
        stats.push({ label: 'Registered', value: info.height != null ? blockLink(info.height) : '<span class="muted">-</span>' });
        stats.push({ label: 'Expires', value: info.expirationHeight ? blockLink(info.expirationHeight) : '<span class="muted">Never</span>' });
      } else if (info.state === 'EXPIRED') {
        stats.push({ label: 'Minimum Bid', value: info.minimumBid ? formatFBC(info.minimumBid) : '<span class="muted">-</span>' });
        stats.push({ label: 'Expired', value: info.expirationHeight > 0 ? blockLink(info.expirationHeight) : '<span class="muted">-</span>' });
      } else {
        stats.push({ label: 'Registered', value: info.height > 0 ? blockLink(info.height) : '<span class="muted">-</span>' });
        stats.push({ label: 'Expires', value: info.expirationHeight ? blockLink(info.expirationHeight) : '<span class="muted">-</span>' });
      }
      var colCount = stats.length;
      var gridClass = colCount >= 4 ? ' fb-stats-4' : colCount === 2 ? ' fb-stats-2' : colCount === 1 ? ' fb-stats-1' : '';
      var statsHTML = '<div class="fb-stats' + gridClass + '">' +
        stats.map(function(s) {
          return '<div class="fb-stat"><div class="fb-stat-label">' + s.label + '</div><div class="fb-stat-value">' + s.value + '</div></div>';
        }).join('') + '</div>';

      // Auction timeline
      var timelineHTML = '';
      var hasAuction = info.height > 0 && info.auction && !info.registered && ['OPENING', 'BIDDING', 'REVEAL', 'CLOSED'].includes(info.state);
      if (hasAuction) {
        var chainRes = await rpc('getblockchaininfo', [], { wallet: null });
        var tipHeight = chainRes.result ? chainRes.result.blocks : 0;
        var stages = [
          { label: 'Open', block: info.height, state: 'OPENING' },
          { label: 'Bidding', block: info.auction.openEnd, state: 'BIDDING' },
          { label: 'Reveal', block: info.auction.biddingEnd, state: 'REVEAL' },
          { label: 'Closed', block: info.auction.revealEnd, state: 'CLOSED' },
        ];
        var currentStage = info.currentStage || info.state;
        var stateOrder = { OPENING: 0, BIDDING: 1, REVEAL: 2, CLOSED: 3, TRANSFER: 3, REVOKED: 3 };
        var activeIdx = stateOrder[currentStage] != null ? stateOrder[currentStage] : -1;
        // Fill: completed stage gaps fully + current stage gap's progress
        // 4 dots, 3 gaps. Track is 75% of container width. Each gap = 75/3 = 25%.
        var numGaps = 3;
        var gapWidth = 75 / numGaps; // 25% each
        var completedGaps = Math.max(activeIdx, 0);
        var fillWidth = Math.min(completedGaps * gapWidth + (info.progress || 0) * gapWidth, 75).toFixed(2);
        var stageHTML = stages.map(function(s, i) {
          var done = i < activeIdx;
          var active = i === activeIdx;
          var cls = done ? 'tl-done' : active ? 'tl-active' : 'tl-future';
          var isPast = s.block <= tipHeight;
          var timeStr = isPast ? blocksToTime(tipHeight - s.block) + ' ago' : '~' + blocksToTime(s.block - tipHeight);
          return '<div class="tl-stage ' + cls + '">' +
            '<div class="tl-dot"></div>' +
            '<div class="tl-label">' + s.label + '</div>' +
            '<a href="' + EXPLORER + '/block/' + s.block + '" target="_blank" class="tl-block">Block ' + Number(s.block).toLocaleString() + '</a>' +
            '<div class="tl-time">' + timeStr + '</div>' +
          '</div>';
        }).join('');
        timelineHTML = '<div class="timeline"><div class="tl-track"></div><div class="tl-fill" style="width:' + fillWidth + '%"></div><div class="tl-stages">' + stageHTML + '</div></div>';
      }

      var dnssecPlaceholder = (dnssec && ['INACTIVE', 'OPENING', 'BIDDING', 'EXPIRED'].includes(info.state))
        ? '<div id="nd-dnssec-setup"><div class="modal-loading">Loading DNSSEC info...</div></div>' : '';
      var belowStats = timelineHTML + dnssecPlaceholder;
      var belowStatsHTML = belowStats ? '<div class="fb-list stats-pad">' + belowStats + '</div>' : '';
      document.getElementById('nd-info').innerHTML =
        '<div class="section-card section-card-flush">' + statsHTML + belowStatsHTML + '</div>';

      // Fetch DNSSEC setup info
      if (dnssec && ['INACTIVE', 'OPENING', 'BIDDING', 'EXPIRED'].includes(info.state)) {
        try {
          const proofRes = await rpc('getdnssecproof', [name]);
          if (proofRes.result) {
            const p = proofRes.result;
            document.getElementById('nd-dnssec-setup').innerHTML =
              '<div class="dnssec-info">' +
              '<div class="section-label">DNSSEC Proof Required</div>' +
              '<p class="muted">This is a ' + esc(info.restriction) + ' name. To open or bid, add a DNS TXT record and enable DNSSEC on your domain:</p>' +
              '<div class="dns-record"><span class="detail-label">Name:</span><code>' + esc(p.subdomain) + '</code></div>' +
              '<div class="dns-record"><span class="detail-label">Type:</span><code>' + esc(p.type) + '</code></div>' +
              '<div class="dns-record"><span class="detail-label">Value:</span><code>' + esc(p.record) + '</code></div>' +
              '</div>';
          }
        } catch(e) {
          document.getElementById('nd-dnssec-setup').innerHTML = '<div class="error-msg">Could not load DNSSEC info.</div>';
        }

      }

      // Action section — open auction or place bid
      const bidEl = document.getElementById('nd-bid-section');
      var pendingBanner = pending
        ? '<div class="section-card"><div class="pending-notice"><span class="badge badge-purple">PENDING</span> ' + esc(pending.action) + ' transaction waiting for confirmation.</div></div>'
        : '';
      const minBidFBC = info.minimumBid ? info.minimumBid / 1000000 : 0;
      var isUpcoming = info.rolloutHeight && info.rolloutHeight > _esLastHeight;
      if (info.state === 'INACTIVE' && !isBlacklisted && !isUpcoming && !pending && !info.parentIssue) {
        var openFields = '';
        if (dnssec) {
          openFields = '<div class="bid-fields">' +
            '<div class="bid-field"><label>Domain</label><input type="text" id="nd-domain" placeholder="yourdomain.com" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false"></div>' +
            '<div class="btn-group-end">' +
              '<div class="btn" id="btn-nd-verify">Verify DNSSEC</div>' +
              '<div class="btn primary" id="btn-nd-open" data-name="' + esc(name) + '">Open Auction</div>' +
            '</div>' +
            '</div>';
        } else {
          openFields = '<div class="bid-fields">' +
            '<div class="btn primary self-end" id="btn-nd-open" data-name="' + esc(name) + '">Open Auction</div>' +
            '</div>';
        }
        bidEl.innerHTML = '<div class="section-card"><div class="section-label">Open Auction</div>' + openFields + '<div id="nd-action-status"></div></div>';
      } else if (info.state === 'BIDDING') {
        let minNote = minBidFBC > 0 ? '<div class="muted info-text">Minimum bid: ' + minBidFBC + '</div>' : '';
        let bidFields = minNote + '<div class="bid-fields">' +
          '<div class="bid-field"><label>Bid</label><input type="text" id="bid-amount" placeholder="' + Math.max(minBidFBC, 100) + '" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false"></div>' +
          '<div class="bid-field"><label>Lockup</label><input type="text" id="bid-lockup" placeholder="' + Math.max(minBidFBC, 150) + '" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false"></div>';
        if (dnssec) {
          bidFields += '<div class="bid-field"><label>Domain</label><input type="text" id="bid-domain" placeholder="yourdomain.com" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false"></div>';
        }
        bidFields += '<div class="btn primary self-end" id="btn-nd-bid">Place Bid</div>' +
          '</div><div id="nd-bid-status"></div>';
        bidEl.innerHTML = '<div class="section-card"><div class="section-label">Place Bid</div>' + bidFields + '</div>';
      } else if (info.state === 'PENDING' && !pending) {
        bidEl.innerHTML = '<div class="section-card"><div class="pending-notice"><span class="badge badge-purple">PENDING</span> Transaction is pending confirmation in the mempool.</div></div>';
      } else if (info.state === 'OPENING') {
        bidEl.innerHTML = '<div class="section-card"><span class="empty-state">Auction is still in opening phase. Bidding starts at ' + blockLink(info.auction.openEnd) + '.</span></div>';
      } else if (info.state === 'REVEAL') {
        var ownUnrevealed = bids.filter(function(b) { return b.own && !b.revealed; });
        var revealHtml = '';
        if (ownUnrevealed.length > 0) {
          revealHtml = '<div class="section-card">' +
            '<div class="action-notice"><span class="badge badge-yellow">Action needed</span> You have ' + ownUnrevealed.length + ' unrevealed bid' + (ownUnrevealed.length > 1 ? 's' : '') + '.</div>' +
            '<div class="name-result-actions"><div class="btn primary" id="btn-nd-reveal" data-name="' + esc(name) + '">Reveal Bids</div></div>' +
            '<div id="nd-reveal-status"></div></div>';
        }
        bidEl.innerHTML = revealHtml;
      } else if (info.state === 'TRANSFER') {
        bidEl.innerHTML = '';
      } else if (info.state === 'CLOSED') {
        var ownBids = bids.filter(function(b) { return b.own || b.revealed; });
        var ownRevealed = ownBids.filter(function(b) { return b.revealed; });
        if (info.registered) {
          if (ownRevealed.length > 0) {
            bidEl.innerHTML = '<div class="section-card">' +
              '<div class="name-result-actions"><div class="btn" id="btn-nd-redeem" data-name="' + esc(name) + '">Redeem Losing Bids</div></div>' +
              '<div id="nd-redeem-status"></div></div>';
          } else {
            bidEl.innerHTML = '';
          }
        } else if (info.canRegister) {
          bidEl.innerHTML = '<div class="section-card">' +
            '<div class="action-notice"><span class="badge badge-yellow">Action needed</span> Auction closed. Register your name to claim it.</div>' +
            '<div class="name-result-actions">' +
            '<div class="btn primary" id="btn-nd-register" data-name="' + esc(name) + '">Register Name</div>' +
            '</div>' +
            '<div id="nd-register-status"></div></div>';
        } else if (ownBids.length > 0) {
          bidEl.innerHTML = '<div class="section-card"><span class="empty-state">Auction is closed. You did not reveal your bids in time.</span></div>';
        } else {
          bidEl.innerHTML = '';
        }
      } else if (info.state === 'EXPIRED' && !isBlacklisted && !pending) {
        var expiredFields = '';
        if (dnssec) {
          expiredFields = '<div class="bid-fields">' +
            '<div class="bid-field"><label>Domain</label><input type="text" id="nd-domain" placeholder="yourdomain.com" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false"></div>' +
            '<div class="btn-group-end">' +
              '<div class="btn" id="btn-nd-verify">Verify DNSSEC</div>' +
              '<div class="btn primary" id="btn-nd-open" data-name="' + esc(name) + '">Open Auction</div>' +
            '</div>' +
            '</div>';
        } else {
          expiredFields = '<div class="bid-fields">' +
            '<div class="btn primary self-end" id="btn-nd-open" data-name="' + esc(name) + '">Open Auction</div>' +
            '</div>';
        }
        bidEl.innerHTML = '<div class="section-card"><div class="section-label">Re-open Auction</div>' +
          expiredFields + '<div id="nd-action-status"></div></div>';
      } else {
        bidEl.innerHTML = '';
      }
      if (pendingBanner) {
        bidEl.style.display = 'flex';
        bidEl.style.flexDirection = 'column';
        bidEl.style.gap = '12px';
        bidEl.innerHTML = pendingBanner + bidEl.innerHTML;
      }

      // Bid button handler
      var bidBtn = document.getElementById('btn-nd-bid');
      if (bidBtn) {
        bidBtn.addEventListener('click', async function() {
          const bidAmt = parseFloat(document.getElementById('bid-amount').value);
          const lockAmt = parseFloat(document.getElementById('bid-lockup').value);
          const st = document.getElementById('nd-bid-status');
          if (isNaN(bidAmt) || bidAmt <= 0) {
            st.innerHTML = '<div class="error-msg">Enter a valid bid amount.</div>';
            return;
          }
          if (minBidFBC > 0 && bidAmt < minBidFBC) {
            st.innerHTML = '<div class="error-msg">Bid too low. Minimum is ' + minBidFBC + '.</div>';
            return;
          }
          if (isNaN(lockAmt) || lockAmt < bidAmt) {
            st.innerHTML = '<div class="error-msg">Lockup must be \u2265 bid amount.</div>';
            return;
          }
          const params = [name, bidAmt, lockAmt];
          if (dnssec) {
            const domain = document.getElementById('bid-domain').value.trim();
            if (!domain) { st.innerHTML = '<div class="error-msg">Enter your domain name for DNSSEC proof.</div>'; return; }
            params.push(domain);
          }
          if (!await showConfirm('Place bid of ' + bidAmt + ' (lockup ' + lockAmt + ') on "' + name + '"?')) return;
          st.innerHTML = '<div class="modal-loading">Placing bid...</div>';
          try {
            const r = { result: await walletSend('sendbid', params) };
            if (r.error) throw new Error(r.error);
            st.innerHTML = '';
            showToast('Bid placed', r.result.txid || r.result);
            setPendingNameAction(name, 'Bid', r.result.txid || r.result, { bidAmount: bidAmt * 1e6, lockupAmount: lockAmt * 1e6 });
            openNameDetail(name);
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      }

      // Reveal button handler
      var revealBtn = document.getElementById('btn-nd-reveal');
      if (revealBtn) {
        revealBtn.addEventListener('click', async function() {
          var st = document.getElementById('nd-reveal-status');
          if (!await showConfirm('Reveal your bids on "' + name + '"?')) return;
          st.innerHTML = '<div class="modal-loading">Revealing bids...</div>';
          try {
            var r = { result: await walletSend('sendreveal', [name]) };
            if (r.error) throw new Error(r.error);
            var txids = r.result.txids || [r.result.txid || r.result];
            st.innerHTML = '';
            showToast('Bids revealed (' + txids.length + ' tx)', txids[0]);
            setPendingNameAction(name, 'Reveal', txids[0]);
            openNameDetail(name);
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      }

      // Redeem button handler
      var redeemBtn = document.getElementById('btn-nd-redeem');
      if (redeemBtn) {
        redeemBtn.addEventListener('click', async function() {
          var st = document.getElementById('nd-redeem-status');
          if (!await showConfirm('Redeem losing bids on "' + name + '"?')) return;
          st.innerHTML = '<div class="modal-loading">Redeeming...</div>';
          try {
            var r = { result: await walletSend('sendredeem', [name]) };
            if (r.error) throw new Error(r.error);
            var txids = r.result.txids || [r.result.txid || r.result];
            st.innerHTML = '';
            showToast('Bids redeemed (' + txids.length + ' tx)', txids[0]);
            setPendingNameAction(name, 'Redeem', txids[0]);
            openNameDetail(name);
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      }

      // Register button handler
      var registerBtn = document.getElementById('btn-nd-register');
      if (registerBtn) {
        registerBtn.addEventListener('click', async function() {
          var st = document.getElementById('nd-register-status');
          if (!await showConfirm('Register the name "' + name + '"?')) return;
          st.innerHTML = '<div class="modal-loading">Registering name...</div>';
          try {
            var r = { result: await walletSend('sendregister', [name]) };
            if (r.error) throw new Error(r.error);
            st.innerHTML = '';
            showToast('Name registered', r.result.txid || r.result);
            setPendingNameAction(name, 'Register', r.result.txid || r.result);
            openNameDetail(name);
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      }

      // DNS records management — only for registered names we own
      // DNS records management — only for registered names we own
      const recordsEl = document.getElementById('nd-records');
      if (isMine && info.registered && info.state !== 'TRANSFER') {
        // Field definitions per record type
        var recordTypes = {
          NS:     { fields: [{ key: 'ns', label: 'Nameserver', ph: 'ns1.example.' }] },
          A:      { fields: [{ key: 'address', label: 'IPv4 address', ph: '1.2.3.4' }] },
          AAAA:   { fields: [{ key: 'address', label: 'IPv6 address', ph: '2001:db8::1' }] },
          CNAME:  { fields: [{ key: 'target', label: 'Target', ph: 'example.' }] },
          TXT:    { fields: [{ key: 'txt', label: 'Text', ph: 'v=spf1 ...' }] },
          MX:     { fields: [
            { key: 'preference', label: 'Priority', ph: '10', size: 'sm', num: true },
            { key: 'exchange', label: 'Mail server', ph: 'mail.example.' }
          ]},
          GLUE4:  { fields: [
            { key: 'ns', label: 'Nameserver', ph: 'ns1.example.' },
            { key: 'address', label: 'IPv4 address', ph: '1.2.3.4' }
          ]},
          GLUE6:  { fields: [
            { key: 'ns', label: 'Nameserver', ph: 'ns1.example.' },
            { key: 'address', label: 'IPv6 address', ph: '2001:db8::1' }
          ]},
          SYNTH4: { fields: [{ key: 'address', label: 'IPv4 address', ph: '1.2.3.4' }] },
          SYNTH6: { fields: [{ key: 'address', label: 'IPv6 address', ph: '2001:db8::1' }] },
          DS:     { fields: [
            { key: 'keyTag', label: 'Key tag', ph: '12345', size: 'sm', num: true },
            { key: 'algorithm', label: 'Algorithm', size: 'sm', num: true, options: [
              { value: '8', label: '8' }, { value: '13', label: '13' },
              { value: '14', label: '14' }, { value: '15', label: '15' },
              { value: '16', label: '16' }
            ]},
            { key: 'digestType', label: 'Digest type', size: 'sm', num: true, options: [
              { value: '2', label: '2' }, { value: '4', label: '4' },
              { value: '1', label: '1' }
            ]},
            { key: 'digest', label: 'Digest', ph: 'abc123...' }
          ]},
          TLSA:   { fields: [
            { key: 'port', label: 'Port', ph: '443', size: 'sm', num: true },
            { key: 'protocol', label: 'Protocol', size: 'sm', num: true, options: [
              { value: '6', label: 'TCP' }, { value: '17', label: 'UDP' }, { value: '132', label: 'SCTP' }
            ]},
            { key: '_rest', label: 'Usage Selector MatchType Cert', ph: '3 1 1 abc123...' }
          ]},
          CAA:    { fields: [
            { key: 'flags', label: 'Flags', size: 'sm', num: true, options: [
              { value: '0', label: '0' }, { value: '128', label: '128' }
            ]},
            { key: 'tag', label: 'Tag', size: 'md', options: [
              { value: 'issue', label: 'issue' }, { value: 'issuewild', label: 'issuewild' },
              { value: 'iodef', label: 'iodef' }
            ]},
            { key: 'value', label: 'Value', ph: 'letsencrypt.org' }
          ]},
          WALLET: { fields: [{ key: 'address', label: 'Address', ph: 'fb1q...' }] }
        };
        var typeKeys = Object.keys(recordTypes);

        var recordsHtml = '<div class="section-card fb-list">' +
          '<div class="section-label">DNS Records</div>' +
          '<div id="nd-records-list"><div class="modal-loading">Loading records...</div></div>' +
          '<div class="nd-record-form">' +
          '<div class="nd-record-form-top">' +
          '<input type="text" id="nd-rec-sub" placeholder="(root)" class="input-narrow" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false">' +
          '<select id="nd-rec-type">' +
          typeKeys.map(function(k) { return '<option value="' + k + '">' + k + '</option>'; }).join('') +
          '</select>' +
          '<div id="nd-rec-fields" class="nd-rec-fields"></div>' +
          '</div>' +
          '<div class="flex-row gap-8">' +
          '<span class="muted text-md">Subdomain auctions</span>' +
          '<label class="switch"><input type="checkbox" id="nd-auction-subdomains"' + (info.auctionSubdomains ? ' checked' : '') + '><span class="switch-slider"></span></label>' +
          '<span class="flex-1"></span>' +
          '<div class="btn nd-add-btn" id="btn-nd-add-record">Add</div>' +
          '</div>' +
          '<div id="nd-rec-error" class="error-msg hidden"></div>' +
          '</div>' +
          '<div class="name-result-actions">' +
          '<div class="btn primary" id="btn-nd-save-records">Save Records</div>' +
          '</div>' +
          '<div id="nd-records-status"></div>' +
          '</div>';
        recordsEl.innerHTML = recordsHtml;

        function renderFields(type) {
          var def = recordTypes[type];
          if (!def) return;
          var html = def.fields.map(function(f) {
            var cls = 'nd-field';
            if (f.size === 'sm') cls += ' nd-field-sm';
            else if (f.size === 'md') cls += ' nd-field-md';
            var inner;
            if (f.options) {
              inner = '<select data-key="' + f.key + '">' +
                f.options.map(function(o) { return '<option value="' + o.value + '">' + esc(o.label) + '</option>'; }).join('') +
                '</select>';
            } else {
              inner = '<input type="text" data-key="' + f.key + '"' + (f.num ? ' inputmode="numeric"' : '') +
                ' placeholder="' + esc(f.ph) + '" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false">';
            }
            return '<div class="' + cls + '">' + inner + '</div>';
          }).join('');
          document.getElementById('nd-rec-fields').innerHTML = html;
        }
        renderFields('NS');

        document.getElementById('nd-rec-type').addEventListener('change', function() {
          renderFields(this.value);
        });

        // Load existing records
        let currentRecords = [];
        rpc('getnameresource', [name]).then(function(res) {
          if (res.result && res.result.records) {
            currentRecords = res.result.records;
          }
          renderRecordsList();
        }).catch(function() { renderRecordsList(); });

        function renderRecordsList() {
          const listEl = document.getElementById('nd-records-list');
          if (!listEl) return;
          if (currentRecords.length === 0) {
            listEl.innerHTML = '<span class="muted text-md">No records</span>';
            return;
          }

          var rootRecords = [];
          var subGroups = [];
          currentRecords.forEach(function(r, i) {
            if (r.type === 'SUB') { subGroups.push({ rec: r, idx: i }); }
            else { rootRecords.push({ rec: r, idx: i }); }
          });

          var html = '';

          // Root records
          if (rootRecords.length > 0) {
            html += '<div class="nd-zone-group">';
            html += '<div class="nd-zone-label">' + esc(name) + '</div>';
            html += '<table class="names-table nd-records-table">';
            rootRecords.forEach(function(item) {
              html += '<tr><td>' + esc(item.rec.type) + '</td><td class="record-value">' + esc(formatRecordValue(item.rec)) + '</td>' +
                '<td><span class="record-delete" data-idx="' + item.idx + '">&times;</span></td></tr>';
            });
            html += '</table></div>';
          }

          // Subdomain groups
          subGroups.forEach(function(item) {
            html += '<div class="nd-zone-group">';
            html += '<div class="nd-zone-label"><span>' + esc(item.rec.name) + '.' + esc(name) +
              '</span><span class="record-delete" data-idx="' + item.idx + '">&times;</span></div>';
            html += '<table class="names-table nd-records-table">';
            (item.rec.records || []).forEach(function(sr, j) {
              html += '<tr><td>' + esc(sr.type) + '</td><td class="record-value">' + esc(formatRecordValue(sr)) + '</td>' +
                '<td><span class="record-delete-sub" data-idx="' + item.idx + '" data-sub="' + j + '">&times;</span></td></tr>';
            });
            html += '</table></div>';
          });

          listEl.innerHTML = html;

          listEl.querySelectorAll('.record-delete').forEach(function(el) {
            el.addEventListener('click', function() {
              currentRecords.splice(parseInt(el.dataset.idx), 1);
              renderRecordsList();
            });
          });
          listEl.querySelectorAll('.record-delete-sub').forEach(function(el) {
            el.addEventListener('click', function() {
              var idx = parseInt(el.dataset.idx);
              var sub = parseInt(el.dataset.sub);
              currentRecords[idx].records.splice(sub, 1);
              if (currentRecords[idx].records.length === 0) {
                currentRecords.splice(idx, 1);
              }
              renderRecordsList();
            });
          });
        }

        function formatRecordValue(r) {
          switch(r.type) {
            case 'NS': return r.ns;
            case 'A': case 'AAAA': case 'SYNTH4': case 'SYNTH6': return r.address;
            case 'CNAME': return r.target;
            case 'TXT': return (r.txt || []).join(' ');
            case 'MX': return r.preference + ' ' + r.exchange;
            case 'GLUE4': case 'GLUE6': return r.ns + ' ' + r.address;
            case 'DS': return r.keyTag + ' ' + r.algorithm + ' ' + r.digestType + ' ' + (r.digest || '');
            case 'TLSA': var proto = {6:'TCP',17:'UDP',132:'SCTP'}[r.protocol] || r.protocol; return ':' + r.port + '/' + proto + ' ' + r.usage + ' ' + r.selector + ' ' + r.matchingType + ' ' + (r.certificate || '');
            case 'CAA': return r.flags + ' ' + r.tag + ' ' + r.value;
            case 'WALLET': return r.address;
            case 'SUB': return r.name + ' (' + (r.records || []).length + ' records)';
            default: return JSON.stringify(r);
          }
        }

        function collectFields() {
          var type = document.getElementById('nd-rec-type').value;
          var def = recordTypes[type];
          if (!def) return null;
          var record = { type: type };
          var inputs = document.querySelectorAll('#nd-rec-fields input, #nd-rec-fields select');
          var hasValue = false;
          inputs.forEach(function(inp) {
            var key = inp.dataset.key;
            var val = inp.value.trim();
            if (val) hasValue = true;
            if (key === '_rest' && type === 'TLSA') {
              var parts = val.split(/\s+/);
              record.usage = parseInt(parts[0]) || 0;
              record.selector = parseInt(parts[1]) || 0;
              record.matchingType = parseInt(parts[2]) || 0;
              record.certificate = parts.slice(3).join('') || '';
              return;
            }
            var fieldDef = def.fields.find(function(f) { return f.key === key; });
            if (fieldDef && fieldDef.num) {
              record[key] = parseInt(val) || 0;
            } else if (key === 'txt') {
              record[key] = [val];
            } else {
              record[key] = val;
            }
          });
          return hasValue ? record : null;
        }

        function isValidHostname(s) {
          var name = s.endsWith('.') ? s.slice(0, -1) : s;
          if (!name) return false;
          var labels = name.split('.');
          for (var i = 0; i < labels.length; i++) {
            var l = labels[i];
            if (l.length < 1 || l.length > 63) return false;
            if (l.startsWith('-') || l.endsWith('-')) return false;
            if (!/^[a-zA-Z0-9_-]+$/.test(l)) return false;
          }
          return true;
        }

        function isValidIPv4(s) { return /^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(s) && s.split('.').every(function(p) { return parseInt(p) <= 255; }); }
        function isValidIPv6(s) { return /^[0-9a-fA-F:]+$/.test(s) && s.split(':').length >= 3; }
        function isValidHex(s) { return s.length > 0 && /^[0-9a-fA-F]+$/.test(s); }
        function isValidAddress(s) { return /^fb1q[a-z0-9]{38,}$/.test(s); }

        function validateRecord(record) {
          var t = record.type;
          if (t === 'WALLET') {
            if (!record.address) return 'Address is required';
            if (!isValidAddress(record.address)) return 'Invalid address (expected fb1q...)';
          } else if (t === 'NS') {
            if (!isValidHostname(record.ns)) return 'Invalid nameserver hostname';
          } else if (t === 'A' || t === 'SYNTH4') {
            if (!isValidIPv4(record.address)) return 'Invalid IPv4 address';
          } else if (t === 'AAAA' || t === 'SYNTH6') {
            if (!isValidIPv6(record.address)) return 'Invalid IPv6 address';
          } else if (t === 'GLUE4') {
            if (!isValidHostname(record.ns)) return 'Invalid nameserver hostname';
            if (!isValidIPv4(record.address)) return 'Invalid IPv4 address';
          } else if (t === 'GLUE6') {
            if (!isValidHostname(record.ns)) return 'Invalid nameserver hostname';
            if (!isValidIPv6(record.address)) return 'Invalid IPv6 address';
          } else if (t === 'CNAME') {
            if (!isValidHostname(record.target)) return 'Invalid target hostname';
          } else if (t === 'MX') {
            if (!isValidHostname(record.exchange)) return 'Invalid mail server hostname';
          } else if (t === 'TXT') {
            if (!record.txt || !record.txt[0]) return 'Text value is required';
          } else if (t === 'DS') {
            if (!record.digest || !isValidHex(record.digest)) return 'Digest must be valid hex';
          } else if (t === 'TLSA') {
            if (!record.certificate || !isValidHex(record.certificate)) return 'Certificate must be valid hex';
          } else if (t === 'CAA') {
            if (!record.value) return 'Value is required';
          }
          return null;
        }

        // Add record button
        document.getElementById('btn-nd-add-record').addEventListener('click', function() {
          var errEl = document.getElementById('nd-rec-error');
          errEl.classList.add('hidden');
          var record = collectFields();
          if (!record) return;
          var err = validateRecord(record);
          if (err) { errEl.textContent = err; errEl.classList.remove('hidden'); return; }
          var sub = (document.getElementById('nd-rec-sub').value || '').trim();
          if (sub) {
            if (sub.length > 63 || !/^[a-zA-Z0-9_-]+$/.test(sub)) {
              errEl.textContent = 'Invalid subdomain name';
              errEl.classList.remove('hidden');
              return;
            }
            // Find existing SUB for this subdomain or create one
            var existing = currentRecords.find(function(r) {
              return r.type === 'SUB' && r.name.toLowerCase() === sub.toLowerCase();
            });
            if (existing) {
              existing.records.push(record);
            } else {
              currentRecords.push({ type: 'SUB', name: sub, records: [record] });
            }
          } else {
            currentRecords.push(record);
          }
          // Clear inputs
          document.querySelectorAll('#nd-rec-fields input').forEach(function(inp) { inp.value = ''; });
          renderRecordsList();
        });

        // Save records button
        document.getElementById('btn-nd-save-records').addEventListener('click', async function() {
          var st = document.getElementById('nd-records-status');
          var resourceJSON = { records: currentRecords };
          var allowSub = document.getElementById('nd-auction-subdomains').checked;
          if (allowSub && !info.auctionSubdomains) {
            if (!await showConfirm('Enable subdomain auctions for "' + name + '"?\n\nThis allows others to register subdomains of your name. This cannot be undone.')) return;
          }
          if (!await showConfirm('Update DNS records for "' + name + '"?')) return;
          st.innerHTML = '<div class="modal-loading">Updating records...</div>';
          try {
            var params = [name, resourceJSON, { auctionSubdomains: allowSub }];
            var r = { result: await walletSend('sendupdate', params) };
            if (r.error) throw new Error(r.error);
            st.innerHTML = '';
            showToast('Records updated', r.result.txid || r.result);
            setPendingNameAction(name, 'Update', r.result.txid || r.result);
            openNameDetail(name);
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      } else {
        recordsEl.innerHTML = '';
      }

      // Transfer section — below records, only if we own the name
      var transferEl = document.getElementById('nd-transfer-section');
      if (isMine && info.state === 'TRANSFER') {
        transferEl.innerHTML = '<div class="fb-list">' +
          '<div class="section-card"><div class="section-label">Transfer in Progress</div>' +
          '<div class="muted info-text">This name has a pending transfer. Finalize to complete it, or cancel to keep the name.</div>' +
          '<div class="bid-fields">' +
            '<div class="btn primary self-end" id="btn-nd-finalize" data-name="' + esc(name) + '">Finalize Transfer</div>' +
            '<div class="btn self-end" id="btn-nd-cancel-transfer" data-name="' + esc(name) + '">Cancel Transfer</div>' +
          '</div><div id="nd-transfer-status"></div></div>' +
          '<div class="section-card">' +
            '<div class="section-label">Danger Zone</div>' +
            '<div class="muted info-text">Revoking permanently burns this name. It will go back to auction and anyone can bid on it.</div>' +
            '<div class="btn danger" id="btn-nd-revoke" data-name="' + esc(name) + '">Revoke Name</div>' +
            '<div id="nd-revoke-status"></div>' +
          '</div></div>';
      } else if (isMine && info.state === 'CLOSED' && info.registered) {
        transferEl.innerHTML = '<div class="fb-list">' +
          '<div class="section-card"><div class="section-label">Transfer Name</div>' +
          '<div class="bid-fields">' +
            '<div class="bid-field"><label>Recipient</label><input type="text" id="nd-transfer-to" placeholder="Address or name" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false"></div>' +
            '<div class="btn primary self-end" id="btn-nd-transfer" data-name="' + esc(name) + '">Transfer</div>' +
          '</div><div id="nd-transfer-status"></div></div>' +
          '<div class="section-card">' +
            '<div class="section-label">Danger Zone</div>' +
            '<div class="muted info-text">Revoking permanently burns this name. It will go back to auction and anyone can bid on it.</div>' +
            '<div class="btn danger" id="btn-nd-revoke" data-name="' + esc(name) + '">Revoke Name</div>' +
            '<div id="nd-revoke-status"></div>' +
          '</div></div>';
      } else {
        transferEl.innerHTML = '';
      }

      // Open auction button (rendered in nd-bid-section above)
      var openBtn = document.getElementById('btn-nd-open');
      if (openBtn) {
        openBtn.addEventListener('click', async () => {
          const st = document.getElementById('nd-action-status');
          const params = [name];
          if (dnssec) {
            const domain = document.getElementById('nd-domain').value.trim();
            if (!domain) { st.innerHTML = '<div class="error-msg">Enter your domain name for DNSSEC proof.</div>'; return; }
            params.push(domain);
          }
          if (!await showConfirm('Open auction for "' + name + '"?')) return;
          st.innerHTML = '<div class="modal-loading">Opening auction...</div>';
          try {
            const r = { result: await walletSend('sendopen', params) };
            if (r.error) throw new Error(r.error);
            st.innerHTML = '';
            showToast('Auction opened', r.result.txid || r.result);
            setPendingNameAction(name, 'Open', r.result.txid || r.result);
            openNameDetail(name);
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      }

      // Verify DNSSEC button
      var verifyBtn = document.getElementById('btn-nd-verify');
      if (verifyBtn) {
        verifyBtn.addEventListener('click', async () => {
          const domain = document.getElementById('nd-domain').value.trim();
          const st = document.getElementById('nd-action-status');
          if (!domain) { st.innerHTML = '<div class="error-msg">Enter your domain name.</div>'; return; }
          st.innerHTML = '<div class="modal-loading">Verifying DNSSEC proof...</div>';
          try {
            const r = await rpc('verifydnssecproof', [name, domain]);
            if (r.error) throw new Error(r.error);
            if (r.result.valid) {
              var addr = r.result.bindingAddress;
              var valRes = await rpc('validateaddress', [addr]);
              var isMine = valRes.result && valRes.result.ismine;
              if (isMine) {
                st.innerHTML = '<div class="success-msg">DNSSEC proof valid! Bound to ' + esc(addr) + '</div>';
              } else {
                st.innerHTML = '<div class="error-msg">DNSSEC proof valid but bound to ' + esc(addr) + ' which is not in this wallet.</div>';
              }
            } else {
              st.innerHTML = '<div class="error-msg">Proof invalid: ' + esc(r.result.message || 'unknown error') + '</div>';
            }
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      }

      // Transfer name button
      var transferBtn = document.getElementById('btn-nd-transfer');
      if (transferBtn) {
        transferBtn.addEventListener('click', async function() {
          var recipient = document.getElementById('nd-transfer-to').value.trim();
          var st = document.getElementById('nd-transfer-status');
          if (!recipient) { st.innerHTML = '<div class="error-msg">Enter a recipient address or name.</div>'; return; }
          // Resolve name to address if needed
          try {
            var resolved = await resolveRecipient(recipient);
            var addr = resolved.address;
            var display = resolved.name ? resolved.name + ' (' + addr.slice(0, 14) + '...)' : addr.slice(0, 20) + '...';
            if (!await showConfirm('Transfer "' + name + '" to ' + display + '?')) return;
            st.innerHTML = '<div class="modal-loading">Initiating transfer...</div>';
            var r = { result: await walletSend('sendtransfer', [name, addr]) };
            if (r.error) throw new Error(r.error);
            st.innerHTML = '';
            showToast('Transfer initiated', r.result.txid || r.result);
            setPendingNameAction(name, 'Transfer', r.result.txid || r.result);
            openNameDetail(name);
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      }

      // Finalize transfer button
      var finalizeBtn = document.getElementById('btn-nd-finalize');
      if (finalizeBtn) {
        finalizeBtn.addEventListener('click', async function() {
          var st = document.getElementById('nd-transfer-status');
          if (!await showConfirm('Finalize transfer of "' + name + '"?')) return;
          st.innerHTML = '<div class="modal-loading">Finalizing...</div>';
          try {
            var r = { result: await walletSend('sendfinalize', [name]) };
            if (r.error) throw new Error(r.error);
            st.innerHTML = '';
            showToast('Transfer finalized', r.result.txid || r.result);
            setPendingNameAction(name, 'Finalize', r.result.txid || r.result);
            openNameDetail(name);
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      }

      // Cancel transfer button (sends an empty update to cancel pending transfer)
      var cancelTransferBtn = document.getElementById('btn-nd-cancel-transfer');
      if (cancelTransferBtn) {
        cancelTransferBtn.addEventListener('click', async function() {
          var st = document.getElementById('nd-transfer-status');
          if (!await showConfirm('Cancel the pending transfer of "' + name + '"?')) return;
          st.innerHTML = '<div class="modal-loading">Cancelling transfer...</div>';
          try {
            // Fetch current records to preserve them
            var resInfo = await rpc('getnameresource', [name]);
            var currentResource = (resInfo.result && resInfo.result.records) ? resInfo.result : { records: [] };
            var r = { result: await walletSend('sendupdate', [name, currentResource]) };
            if (r.error) throw new Error(r.error);
            st.innerHTML = '';
            showToast('Transfer cancelled', r.result.txid || r.result);
            setPendingNameAction(name, 'Cancel transfer', r.result.txid || r.result);
            openNameDetail(name);
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      }

      // Revoke name button (double confirm — permanently burns the name)
      var revokeBtn = document.getElementById('btn-nd-revoke');
      if (revokeBtn) {
        revokeBtn.addEventListener('click', async function() {
          var st = document.getElementById('nd-revoke-status');
          if (!await showConfirm('Revoke "' + name + '"? This will permanently burn this name.', { danger: true, okText: 'Revoke' })) return;
          if (!await showConfirm('Are you sure? This cannot be undone. The name will go back to auction.', { danger: true, okText: 'Revoke' })) return;
          st.innerHTML = '<div class="modal-loading">Revoking...</div>';
          try {
            var r = { result: await walletSend('sendrevoke', [name]) };
            if (r.error) throw new Error(r.error);
            st.innerHTML = '';
            showToast('Name revoked', r.result.txid || r.result);
            setPendingNameAction(name, 'Revoke', r.result.txid || r.result);
            openNameDetail(name);
          } catch(e) {
            st.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
          }
        });
      }

      // All bids on this name — only show during active auction phases
      const bidsEl = document.getElementById('nd-bids');
      var auctionActive = ['BIDDING', 'REVEAL'].includes(info.state);
      if (!auctionActive) {
        bidsEl.textContent = '';
      } else {
      // Build lookup of our own bids by txHash for cross-referencing
      var ownBidMap = {};
      bids.forEach(function(b) {
        if (b.own && b.outpoint) ownBidMap[b.outpoint.hash + ':' + b.outpoint.index] = b;
      });
      // Build a set of network bid keys for dedup
      var networkKeys = new Set();
      networkBids.forEach(function(b) { networkKeys.add((b.txHash || '') + ':' + (b.outputIndex || 0)); });
      // Enrich network bids with ownership + real bid value from wallet
      networkBids.forEach(function(b) {
        var txid = b.txHash || b.txid || '';
        var idx = b.outputIndex != null ? b.outputIndex : (b.index || 0);
        var key = txid + ':' + idx;
        var own = ownBidMap[key];
        if (own) { b.isOwn = true; b.realValue = own.value; if (own.needsRepair) b._needsRepair = true; }
      });
      // For wallet-only bids, mark all as own and carry repair flag
      bids.forEach(function(b) {
        if (b.own) b.isOwn = true;
        b.realValue = b.value;
        if (b.needsRepair) b._needsRepair = true;
      });
      // Merge only pending (unrevealed, not yet indexed) wallet bids into network list
      if (networkBids.length > 0) {
        bids.forEach(function(b) {
          if (!b.own || !b.outpoint || b.revealed) return;
          var key = b.outpoint.hash + ':' + b.outpoint.index;
          if (!networkKeys.has(key)) {
            networkBids.push({ lockup: b.lockup, txHash: b.outpoint.hash, outputIndex: b.outpoint.index, revealedValue: null, address: '', height: b.height || 0, isOwn: true, realValue: b.value, _needsRepair: b.needsRepair, pending: true });
          }
        });
      }

      const displayBids = (networkBids.length > 0 ? networkBids : bids)
        .sort(function(a, b) {
          var ah = a.height > 0 ? a.height : 0, bh = b.height > 0 ? b.height : 0;
          if (!ah && !bh) return (b.lockup || 0) - (a.lockup || 0);
          if (!ah) return -1;
          if (!bh) return 1;
          return bh - ah;
        });
      if (displayBids.length === 0) {
        bidsEl.textContent = '';
      } else {
        var svgIcon = '<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M6 3H3v10h10v-3"/><path d="M9 2h5v5"/><path d="M14 2L7 9"/></svg>';
        var isBiddingPhase = info.state === 'BIDDING' || info.state === 'OPENING';
        var bidItems = displayBids.map(function(b, idx) {
            var bidAmount = b.isOwn && b.realValue != null ? b.realValue : (b.revealedValue != null ? b.revealedValue : null);
            var bidStr, blindStr;
            if (b._needsRepair && b.isOwn) {
              bidStr = '<a href="#" class="bid-repair-link bid-repair" title="Click to repair bid">?.??</a>';
              blindStr = '<span class="muted">' + formatFBC(0) + '</span>';
            } else if (bidAmount != null) {
              bidStr = formatFBC(bidAmount);
              var blind = b.lockup - bidAmount;
              blindStr = '<span class="muted">' + formatFBC(blind >= 0 ? blind : 0) + '</span>';
            } else {
              bidStr = '<span class="muted">Unknown</span>';
              blindStr = null;
            }
            var valueCol = bidStr;
            var lockupCol = formatFBC(b.lockup);
            var txHash = b.txHash || b.txid || '';
            var isMempool = b.pending || b.height < 0;
            var timeStr, dateStr2;
            if (isMempool) {
              var now = new Date();
              timeStr = now.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
              dateStr2 = now.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
            } else if (b.height > 0) {
              var secondsAgo = (_esLastHeight - b.height) * 60;
              var d = new Date(Date.now() - secondsAgo * 1000);
              timeStr = d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
              dateStr2 = d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
            } else {
              timeStr = '--'; dateStr2 = '';
            }
            var dateCol = timeStr + (dateStr2 ? '<div class="muted text-sm">' + dateStr2 + '</div>' : '');
            var txCol = isMempool ? '<span class="muted">Mempool</span>' : (txHash ? '<a href="' + EXPLORER + '/tx/' + txHash + '" target="_blank" title="View transaction">' + svgIcon + '</a>' : '');
            var ownTag = b.isOwn ? ' <span class="badge badge-blue badge-you">you</span>' : '';
            var numCol = '<span class="muted">' + (displayBids.length - idx) + '</span>' + ownTag;
            return {
              num: numCol, date: dateCol, value: valueCol, lockup: lockupCol, tx: txCol,
              timeStr: timeStr, dateStr: dateStr2, ownTag: ownTag, idx: idx,
              bidStr: bidStr, blindStr: blindStr, isOwn: b.isOwn, lockupRaw: formatFBC(b.lockup)
            };
        });
        // Desktop: table
        var tableHTML = '<table class="names-table bids-table bids-desktop"><tr><th>#</th><th>Time</th><th>Bid</th><th>Lockup</th><th></th></tr>' +
          bidItems.map(function(b) {
            return '<tr><td>' + b.num + '</td><td>' + b.date + '</td><td>' + b.value + '</td><td>' + b.lockup + '</td><td class="text-right">' + b.tx + '</td></tr>';
          }).join('') + '</table>';
        // Mobile: cards
        var cardsHTML = '<div class="bids-mobile">' +
          bidItems.map(function(b) {
            return '<div class="bid-card">' +
              '<div class="bid-card-top"><span>' + b.num + '</span><span>' + b.tx + '</span></div>' +
              '<div class="bid-card-divider"></div>' +
              '<div class="bid-card-grid">' +
                '<div class="bid-card-cell"><div class="bid-card-label">Bid</div><div>' + b.bidStr + '</div></div>' +
                '<div class="bid-card-cell"><div class="bid-card-label">Lockup</div><div>' + b.lockupRaw + '</div></div>' +
              '</div>' +
              '<div class="bid-card-divider"></div>' +
              '<div class="bid-card-footer muted"><span>' + b.timeStr + '</span>' + (b.dateStr ? '<span>' + b.dateStr + '</span>' : '') + '</div>' +
            '</div>';
          }).join('') + '</div>';
        bidsEl.innerHTML = '<div class="section-card"><div class="section-label">Bids (' + displayBids.length + ')</div>' + tableHTML + cardsHTML + '</div>';
      // Handle repair link clicks in bids table
      bidsEl.querySelectorAll('.bid-repair-link').forEach(function(link) {
        link.addEventListener('click', function(e) {
          e.preventDefault();
          var repairBid = bids.find(function(b) { return b.needsRepair && b.own; });
          if (!repairBid) return;
          document.getElementById('repair-name').textContent = name;
          document.getElementById('repair-lockup').innerHTML = 'Lockup: ' + formatFBC(repairBid.lockup);
          document.getElementById('repair-status').textContent = '';
          openModal('repair-modal');
          var submitBtn = document.getElementById('repair-submit');
          var autoBtn = document.getElementById('repair-auto');
          var newSubmit = submitBtn.cloneNode(true);
          var newAuto = autoBtn.cloneNode(true);
          submitBtn.replaceWith(newSubmit);
          autoBtn.replaceWith(newAuto);
          newSubmit.addEventListener('click', async function() {
            var val = document.getElementById('repair-value').value;
            if (!val) return;
            var rs = document.getElementById('repair-status');
            rs.innerHTML = '<div class="modal-loading">Recovering...</div>';
            try {
              var r = await rpc('repairbid', [name, parseFloat(val)]);
              if (r.error) throw new Error(r.error);
              rs.innerHTML = '<div class="success-msg">Bid recovered!</div>';
              setTimeout(function() { closeModal('repair-modal'); openNameDetail(name); refreshOverviewActions(); }, 1500);
            } catch(err) { rs.innerHTML = '<div class="error-msg">' + esc(friendlyError(err.message)) + '</div>'; }
          });
          newAuto.addEventListener('click', async function() {
            var rs = document.getElementById('repair-status');
            rs.innerHTML = '<div class="modal-loading">Auto-recovering...</div>';
            try {
              var r = await rpc('repairbid', [name]);
              if (r.error) throw new Error(r.error);
              rs.innerHTML = '<div class="success-msg">Bid recovered!</div>';
              setTimeout(function() { closeModal('repair-modal'); openNameDetail(name); refreshOverviewActions(); }, 1500);
            } catch(err) { rs.innerHTML = '<div class="error-msg">' + esc(friendlyError(err.message)) + '</div>'; }
          });
        });
      });
      }
      } // end auctionActive
    } catch(e) {
      document.getElementById('nd-info').innerHTML = '<div class="name-result-card"><div class="error-msg">' + esc(friendlyError(e.message)) + '</div></div>';
      document.getElementById('nd-bid-section').innerHTML = '';
      document.getElementById('nd-records').innerHTML = '';
      document.getElementById('nd-transfer-section').innerHTML = '';
      document.getElementById('nd-bids').textContent = '';
    }
    // Restore scroll position on refresh (after DOM rebuild)
    if (isRefresh) main.scrollTop = savedScroll;
  }

  // ---- Auctions ----

  let auctionBids = [];
  let auctionFilter = 'all';

  document.getElementById('auction-filters').addEventListener('click', (e) => {
    const btn = e.target.closest('.filter-btn');
    if (!btn) return;
    auctionFilter = btn.dataset.filter;
    document.querySelectorAll('#auction-filters .filter-btn').forEach(b => b.classList.remove('active'));
    btn.classList.add('active');
    renderAuctions();
  });

  async function loadAuctions() {
    try {
      const [bidsRes, namesRes, auctionsRes] = await Promise.all([
        rpc('getbids'),
        rpc('getnames'),
        rpc('getauctions', [], { wallet: null }),
      ]);
      const allNames = namesRes.result || [];
      const auctionTypes = ['OPEN', 'BID', 'REVEAL', 'REDEEM', 'NONE'];
      const registeredHashes = new Set(allNames
        .filter(n => n.name.state === 'CLOSED' && !auctionTypes.includes(n.covenantType))
        .map(n => n.name.hash));
      // Flatten getbids name object to root-level properties for merging with other sources
      (bidsRes.result || []).forEach(function(b) {
        if (b.name && typeof b.name === 'object') {
          var n = b.name;
          b.nameHash = n.hash;
          b.state = n.state;
          b.name = n.string || '';
        }
      });
      const bids = (bidsRes.result || []).filter(b => (b.own || b.revealed) && !registeredHashes.has(b.nameHash));
      // Chain state lookup — authoritative source of truth for name state
      const chainStateByHash = {};
      allNames.forEach(n => { chainStateByHash[n.name.hash] = n.name.state; });
      // Build map of wallet-tracked nameHashes
      const walletHashes = new Set(bids.map(b => b.nameHash));
      allNames.forEach(n => walletHashes.add(n.name.hash));
      // Merge in opened names that don't have bids yet and aren't registered
      const bidNameHashes = new Set(bids.map(b => b.nameHash));
      const opens = allNames
        .filter(n => !bidNameHashes.has(n.name.hash) && !registeredHashes.has(n.name.hash) && ['PENDING', 'OPENING', 'BIDDING', 'REVEAL', 'CLOSED'].includes(n.name.state))
        .map(n => ({ nameHash: n.name.hash, name: n.name.string, state: n.name.state, value: 0, lockup: 0, revealed: false, isOpen: true, own: true }));
      // Flatten getauctions nested objects for merging with other sources
      const allAuctions = auctionsRes.result || [];
      allAuctions.forEach(function(a) {
        if (a.name && typeof a.name === 'object') {
          var n = a.name;
          a.nameHash = n.hash;
          a.state = n.state;
          a.name = n.string || '';
        }
        if (a.auction) {
          a.height = a.auction.height;
          a.openEnd = a.auction.openEnd;
          a.biddingEnd = a.auction.biddingEnd;
          a.revealEnd = a.auction.revealEnd;
          a.bidCount = a.auction.bidCount;
          a.highestRevealed = a.auction.highestRevealed;
          a.highestLockup = a.auction.highestLockup;
        }
      });
      const auctionByHash = {};
      allAuctions.forEach(a => { auctionByHash[a.nameHash] = a; });
      // Enrich wallet bids and opens with network bid count / highest lockup.
      // Override state with chain state (authoritative) when available.
      bids.forEach(b => {
        if (chainStateByHash[b.nameHash]) b.state = chainStateByHash[b.nameHash];
        var a = auctionByHash[b.nameHash];
        if (a) { b.bidCount = a.bidCount; b.highestRevealed = a.highestRevealed; b.highestLockup = a.highestLockup; }
      });
      opens.forEach(b => {
        if (chainStateByHash[b.nameHash]) b.state = chainStateByHash[b.nameHash];
        var a = auctionByHash[b.nameHash];
        if (a) { b.bidCount = a.bidCount; b.highestRevealed = a.highestRevealed; b.highestLockup = a.highestLockup; }
      });
      // Merge in network auctions not already tracked by the wallet.
      // Fetch real chain state for names we don't have chain state for.
      const unknownNames = allAuctions
        .filter(a => !walletHashes.has(a.nameHash) && !chainStateByHash[a.nameHash] && a.name)
        .map(a => a.name);
      const nameInfoResults = await Promise.all(
        [...new Set(unknownNames)].map(n => rpc('getnameinfo', [n], { wallet: null }).catch(() => null))
      );
      nameInfoResults.forEach(r => {
        if (r && r.result && r.result.nameHash) chainStateByHash[r.result.nameHash] = r.result.state;
      });
      const networkAuctions = allAuctions
        .filter(a => !walletHashes.has(a.nameHash))
        .map(a => ({
          nameHash: a.nameHash,
          name: a.name,
          state: chainStateByHash[a.nameHash] || a.state.toUpperCase(),
          value: 0,
          lockup: 0,
          revealed: false,
          isNetwork: true,
          bidCount: a.bidCount,
          highestRevealed: a.highestRevealed,
          highestLockup: a.highestLockup,
        }));
      // Deduplicate by nameHash — multiple bids on the same name should show as one row
      var merged = [...opens, ...bids, ...networkAuctions];
      var seen = {};
      var deduped = [];
      merged.forEach(function(b) {
        if (!b.nameHash) { deduped.push(b); return; }
        if (seen[b.nameHash]) {
          // Keep the entry with more info (higher lockup, own over network)
          var existing = seen[b.nameHash];
          if (b.own && !existing.own) { deduped[deduped.indexOf(existing)] = b; seen[b.nameHash] = b; }
          else if (b.lockup > existing.lockup) { existing.lockup = b.lockup; }
        } else {
          seen[b.nameHash] = b;
          deduped.push(b);
        }
      });
      auctionBids = deduped.sort((a, b) => (a.name || '').localeCompare(b.name || ''));
      renderAuctions();
    } catch(e) {
      document.getElementById('bid-list').textContent = friendlyError(e.message) || 'Error loading data';
    }
  }

  function renderAuctions() {
    const bids = auctionBids;
    const activeBids = bids.filter(b => ['PENDING', 'OPENING', 'BIDDING', 'REVEAL'].includes(b.state));
    const closedBids = bids.filter(b => b.state === 'CLOSED');
    const needsReveal = bids.filter(b => !b.revealed && b.own && b.state === 'REVEAL' && !b.isNetwork);

    let filtered;
    if (auctionFilter === 'all') filtered = activeBids;
    else if (auctionFilter === 'CLOSED') filtered = closedBids;
    else filtered = bids.filter(b => b.state === auctionFilter);

    document.getElementById('auction-stats').innerHTML = '';

    // Bids
    const bidEl = document.getElementById('bid-list');
    if (filtered.length === 0) {
      bidEl.className = 'empty-state';
      const labels = { all: 'No active auctions', OPENING: 'No names in opening phase', BIDDING: 'No auctions in bidding phase', REVEAL: 'No auctions in reveal phase', CLOSED: 'No closed auctions' };
      bidEl.textContent = labels[auctionFilter] || 'No auctions';
    } else {
      bidEl.className = '';
      var isClosed = auctionFilter === 'CLOSED';
      var lockupHeader = isClosed ? 'Winning Bid' : 'Highest Lockup';
      bidEl.innerHTML = '<table class="names-table"><tr><th>Name</th><th>State</th><th>Bids</th><th>' + lockupHeader + '</th><th></th></tr>' +
        filtered.map(b => {
          var bidCountCol = b.bidCount != null ? b.bidCount : '<span class="muted">--</span>';
          var lockupCol;
          if (b.highestRevealed > 0) {
            lockupCol = formatFBC(b.highestRevealed);
          } else if (b.highestLockup > 0) {
            lockupCol = formatFBC(b.highestLockup);
          } else {
            lockupCol = '<span class="muted">--</span>';
          }
          var actionBtn = '';
          if (b.state === 'REVEAL' && b.own && !b.revealed && !b.isOpen && !b.isNetwork) {
            actionBtn = '<div class="btn primary btn-sm auction-action" data-action="reveal" data-name="' + esc(b.name || '') + '">Reveal</div>';
          } else if (b.state === 'CLOSED' && b.own && b.revealed && !b.isNetwork && !b.forfeited && !b.redeemed) {
            actionBtn = '<div class="btn primary btn-sm auction-action" data-action="register" data-name="' + esc(b.name || '') + '">Register</div>';
          }
          return '<tr>' +
            '<td>' + nameLink(b.name, b.nameHash) + '</td>' +
            '<td>' + stateBadge(b.state) + '</td>' +
            '<td>' + bidCountCol + '</td>' +
            '<td>' + lockupCol + '</td>' +
            '<td>' + actionBtn + '</td>' +
          '</tr>';
        }).join('') +
        '</table>';
    }
  }

  document.getElementById('bid-list').addEventListener('click', async function(e) {
    var btn = e.target.closest('.auction-action');
    if (!btn) return;
    var action = btn.dataset.action;
    var name = btn.dataset.name;
    if (!name) return;
    var labels = { reveal: 'Reveal bids on', register: 'Register', redeem: 'Redeem losing bids on' };
    if (!await showConfirm((labels[action] || action) + ' "' + name + '"?')) return;
    btn.style.opacity = '0.5';
    btn.style.pointerEvents = 'none';
    try {
      var r;
      if (action === 'reveal') {
        r = await rpc('sendreveal', [name]);
      } else if (action === 'register') {
        r = await rpc('sendregister', [name]);
      } else if (action === 'redeem') {
        r = await rpc('sendredeem', [name]);
      }
      if (r && r.error) throw new Error(r.error);
      var txid = r.result && (r.result.txid || (r.result.txids && r.result.txids[0]) || r.result);
      showToast(action.charAt(0).toUpperCase() + action.slice(1) + ' sent', typeof txid === 'string' ? txid : null);
      btn.textContent = 'Done';
      btn.className = 'btn btn-sm';
      setTimeout(function() { loadAuctions(); }, 2000);
    } catch(e2) {
      btn.textContent = 'Error';
      btn.style.opacity = '';
      btn.style.pointerEvents = '';
      setTimeout(function() {
        btn.textContent = action.charAt(0).toUpperCase() + action.slice(1);
      }, 2000);
    }
  });

  // ---- Browser ----
  // Browse now lives entirely in a native modal (see openBrowser above).
  // Nothing to do here — search input is name-lookup only.

  var searchLock = document.getElementById('search-lock');

  // ---- Log ----

  const levels = { 'D': 0, 'I': 1, 'W': 2, 'E': 3 };
  var logRaw = [];
  var logLength = 0;

  function getLogLevel() {
    return localStorage.getItem('logLevel') || 'I';
  }

  function filterLog(lines) {
    const min = levels[getLogLevel()] || 0;
    return lines.filter(line => {
      const m = line.match(/^([DIWE])\)/);
      return m ? (levels[m[1]] || 0) >= min : true;
    });
  }

  async function refreshLog() {
    try {
      logRaw = await __invoke('get_log');
      renderLog();
    } catch(e) {}
  }

  function renderLog() {
    const filtered = filterLog(logRaw);
    const el = document.getElementById('log-output');
    el.textContent = filtered.join('\n');
    if (logRaw.length !== logLength) {
      logLength = logRaw.length;
      el.scrollTop = el.scrollHeight;
    }
  }

  const levelSelect = document.getElementById('setting-loglevel');
  levelSelect.value = getLogLevel();
  levelSelect.addEventListener('change', () => {
    localStorage.setItem('logLevel', levelSelect.value);
    renderLog();
  });
  // Initial log fetch (subsequent lines arrive via SSE)
  refreshLog();

  // ---- RPC Console ----
  (function() {
    var input = document.getElementById('console-input');
    var output = document.getElementById('console-output');
    var history = [];
    var histIdx = -1;
    var firstCommand = true;

    function appendOutput(html) {
      if (firstCommand) { output.innerHTML = ''; firstCommand = false; }
      output.innerHTML += html + '\n';
      output.scrollTop = output.scrollHeight;
    }

    input.addEventListener('keydown', function(e) {
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (histIdx < history.length - 1) { histIdx++; input.value = history[histIdx]; }
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (histIdx > 0) { histIdx--; input.value = history[histIdx]; }
        else { histIdx = -1; input.value = ''; }
      } else if (e.key === 'Enter') {
        var line = input.value.trim();
        if (!line) return;
        input.value = '';
        history.unshift(line);
        histIdx = -1;

        if (line === 'clear') { output.innerHTML = '<span class="muted">Do not paste commands from anyone you don\'t know or trust.</span>'; firstCommand = true; return; }
        if (line === 'help') { appendOutput('<span class="muted">Documentation: <a href="https://fbd.dev" target="_blank">fbd.dev</a></span>'); return; }

        var parts = line.match(/(?:[^\s"]+|"[^"]*")/g) || [];
        var method = parts[0];
        var params = parts.slice(1).map(function(p) {
          // Strip surrounding quotes
          if (p.startsWith('"') && p.endsWith('"')) p = p.slice(1, -1);
          // Parse numbers and booleans
          if (p === 'true') return true;
          if (p === 'false') return false;
          if (p === 'null') return null;
          var n = Number(p);
          if (!isNaN(n) && p !== '') return n;
          // Try parsing as JSON (arrays/objects)
          try { return JSON.parse(p); } catch(e) {}
          return p;
        });

        appendOutput('<span class="console-cmd">&gt; ' + esc(line) + '</span>');

        rpc(method, params).then(function(res) {
          if (res.error) {
            appendOutput('<span class="console-err">' + esc(res.error) + '</span>');
          } else {
            appendOutput(esc(JSON.stringify(res.result, null, 2)));
          }
        }).catch(function(err) {
          appendOutput('<span class="console-err">' + esc(err.message) + '</span>');
        });
      }
    });
  })();

  // ---- Multisig Flow ----

  var currentPstx = '';

  function showMultisigFlow(pstxHex, sigCount) {
    currentPstx = pstxHex;
    var modal = document.getElementById('multisig-modal');
    var pstxEl = document.getElementById('multisig-pstx');
    var importEl = document.getElementById('multisig-import');
    var statusEl = document.getElementById('multisig-status');
    var resultEl = document.getElementById('multisig-result');
    pstxEl.value = pstxHex;
    importEl.value = '';
    var sigs = sigCount || 1;
    var needed = walletMultisigM - sigs;
    var msg = 'Signatures: ' + sigs + ' of ' + walletMultisigM + ' required';
    if (needed > 0) msg += '. Copy the PSTX and send it to ' + (needed === 1 ? 'your cosigner' : needed + ' cosigners') + ' to sign.';
    else msg += '. Ready to broadcast.';
    statusEl.innerHTML = '<div class="muted text-base">' + msg + '</div>';
    resultEl.innerHTML = '';
    modal.classList.remove('hidden');
  }

  document.getElementById('multisig-close').addEventListener('click', function() {
    document.getElementById('multisig-modal').classList.add('hidden');
  });

  document.getElementById('multisig-copy').addEventListener('click', function() {
    navigator.clipboard.writeText(currentPstx).then(function() {
      showToast('Copied to clipboard');
    });
  });

  document.getElementById('multisig-combine').addEventListener('click', async function() {
    var importHex = document.getElementById('multisig-import').value.trim();
    var resultEl = document.getElementById('multisig-result');
    if (!importHex) { resultEl.innerHTML = '<div class="error-msg">Paste a cosigner\'s signed PSTX.</div>'; return; }
    try {
      var r = await rpc('combinetx', [[currentPstx, importHex]], { wallet: null });
      if (r.error) throw new Error(r.error);
      currentPstx = r.result.pstx || r.result.hex || r.result;
      document.getElementById('multisig-pstx').value = currentPstx;
      document.getElementById('multisig-import').value = '';
      var sigs = r.result.signatures || 0;
      var needed = walletMultisigM - sigs;
      var msg = 'Signatures: ' + sigs + ' of ' + walletMultisigM + ' required';
      if (needed > 0) msg += '. Need ' + needed + ' more.';
      else msg += '. Ready to broadcast.';
      document.getElementById('multisig-status').innerHTML = '<div class="muted text-base">' + msg + '</div>';
      resultEl.innerHTML = '<div class="success-msg">Signatures combined.</div>';
      setTimeout(function() { resultEl.innerHTML = ''; }, 3000);
    } catch(e) {
      resultEl.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  });

  document.getElementById('multisig-broadcast').addEventListener('click', async function() {
    var resultEl = document.getElementById('multisig-result');
    try {
      var r = await rpc('broadcasttx', [currentPstx], { wallet: null });
      if (r.error) throw new Error(r.error);
      var txid = r.result.txid || r.result;
      document.getElementById('multisig-modal').classList.add('hidden');
      showToast('Transaction broadcast', typeof txid === 'string' ? txid : null);
    } catch(e) {
      resultEl.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  });

  /// For multisig wallets, intercept the normal send flow:
  /// 1. createtx <action> [params] → unsigned PSTX
  /// 2. signtx <pstx> → partially signed PSTX
  /// 3. Show multisig modal
  async function multisigAction(action, params) {
    var r = await rpc('createtx', [action].concat(params || []));
    if (r.error) throw new Error(r.error);
    var pstxHex = r.result.hex || r.result;
    // Sign with our key
    var s = await rpc('signtx', [pstxHex]);
    if (s.error) throw new Error(s.error);
    var signedHex = s.result.hex || s.result;
    var sigCount = s.result.signatures || 1;
    showMultisigFlow(signedHex, sigCount);
    return { multisig: true, txid: 'pending cosigner signatures' };
  }

  /// Wrapper: for multisig wallets, use createtx+signtx flow.
  /// For regular wallets, call the normal send RPC directly.
  /// Maps send<Action> to createtx <action> params.
  async function walletSend(sendMethod, params) {
    if (!walletIsMultisig) {
      var r = await rpc(sendMethod, params);
      if (r.error) throw new Error(r.error);
      return r.result;
    }
    // Map sendopen → "open", sendbid → "bid", etc.
    var action = sendMethod.replace('send', '');
    return multisigAction(action, params);
  }

  // ---- Wallet Management ----

  async function loadWalletInfo() {
    try {
      const res = await rpc('getwalletinfo');
      if (res.error) return;
      const w = res.result;
      document.getElementById('wallet-info-name').textContent = w.name || '--';
      document.getElementById('wallet-info-type').textContent = w.type || 'regular';
      document.getElementById('wallet-info-address').textContent = w.address || '--';
      document.getElementById('wallet-info-scan').innerHTML = w.scanHeight != null ? blockLink(w.scanHeight) : '--';
      walletEncrypted = !!w.encrypted;
      walletUnlocked = !w.encrypted || !!w.unlocked;
      walletIsMultisig = w.type === 'multisig';
      walletMultisigM = w.m || 0;
      walletMultisigN = w.n || 0;
      updateLockIndicator();

      // Show biometric enable/disable buttons if supported and wallet is encrypted
      var bioEnabled = localStorage.getItem('biometric_' + activeWallet) === '1';
      var enableBtn = document.getElementById('btn-enable-biometric');
      var disableBtn = document.getElementById('btn-disable-biometric');
      if (biometricSupported && walletEncrypted) {
        enableBtn.classList.toggle('hidden', bioEnabled);
        disableBtn.classList.toggle('hidden', !bioEnabled);
      } else {
        enableBtn.classList.add('hidden');
        disableBtn.classList.add('hidden');
      }
    } catch(e) {}
  }

  document.getElementById('btn-enable-biometric').addEventListener('click', async () => {
    var statusEl = document.getElementById('encrypt-status');
    statusEl.innerHTML = '';

    // Use the unlock modal to capture the passphrase
    var passphrase = await new Promise(function(resolve) {
      var overlay = document.getElementById('unlock-modal');
      var input = document.getElementById('unlock-passphrase');
      var okBtn = document.getElementById('unlock-ok');
      var cancelBtn = document.getElementById('unlock-cancel');
      var unlockStatus = document.getElementById('unlock-status');
      input.value = '';
      unlockStatus.innerHTML = '<div class="muted text-base">Enter your passphrase to enable biometric unlock.</div>';
      okBtn.style.opacity = '';
      okBtn.style.pointerEvents = '';
      overlay.classList.remove('hidden');
      setTimeout(function() { input.focus(); }, 50);
      var resolved = false;
      function done(pw) {
        if (resolved) return;
        resolved = true;
        overlay.classList.add('hidden');
        document.removeEventListener('keydown', onKey, true);
        okBtn.replaceWith(okBtn.cloneNode(true));
        cancelBtn.replaceWith(cancelBtn.cloneNode(true));
        resolve(pw);
      }
      async function tryVerify() {
        var pw = input.value;
        if (!pw) { unlockStatus.innerHTML = '<div class="error-msg">Enter a passphrase.</div>'; return; }
        okBtn.style.opacity = '0.5';
        okBtn.style.pointerEvents = 'none';
        unlockStatus.innerHTML = '<div class="modal-loading">Verifying...</div>';
        try {
          var res = await rpc('walletpassphrase', [pw, 300]);
          if (res.error) throw new Error(res.error);
          done(pw);
        } catch(e) {
          unlockStatus.innerHTML = '<div class="error-msg">Wrong passphrase.</div>';
          okBtn.style.opacity = '';
          okBtn.style.pointerEvents = '';
          input.value = '';
          input.focus();
        }
      }
      function onKey(e) {
        if (e.key === 'Enter') { e.preventDefault(); tryVerify(); }
        else if (e.key === 'Escape') done(null);
      }
      document.addEventListener('keydown', onKey, true);
      document.getElementById('unlock-ok').addEventListener('click', tryVerify);
      document.getElementById('unlock-cancel').addEventListener('click', function() { done(null); });
    });

    if (!passphrase) return;

    try {
      var saved = await __invoke('biometric_save', { wallet: activeWallet, passphrase: passphrase });
      if (saved) {
        localStorage.setItem('biometric_' + activeWallet, '1');
        statusEl.innerHTML = '<div class="success-msg">Biometric unlock enabled.</div>';
        setTimeout(function() { statusEl.innerHTML = ''; }, 3000);
        loadWalletInfo();
      } else {
        statusEl.innerHTML = '<div class="error-msg">Failed to save biometric key.</div>';
      }
    } catch(e) {
      // Tauri rejects with the raw Err value — a plain string when the Rust
      // side returns Err(String), so e.message is undefined. Stringify safely.
      var msg = (typeof e === 'string') ? e : (e && e.message) ? e.message : String(e);
      console.error('biometric_save failed:', e);
      statusEl.innerHTML = '<div class="error-msg">' + esc(msg) + '</div>';
    }
  });

  document.getElementById('btn-disable-biometric').addEventListener('click', async () => {
    if (!walletUnlocked) {
      var ok = await requireUnlock();
      if (!ok) return;
    }
    if (!await showConfirm('Disable biometric unlock for this wallet?')) return;
    try {
      await __invoke('biometric_delete', { wallet: activeWallet });
    } catch(e) {}
    localStorage.setItem('biometric_' + activeWallet, '0');
    var statusEl = document.getElementById('encrypt-status');
    statusEl.innerHTML = '<div class="success-msg">Biometric unlock disabled.</div>';
    setTimeout(function() { statusEl.innerHTML = ''; }, 3000);
    loadWalletInfo();
  });

  // ---- QR Code ----

  document.getElementById('qr-show-close').addEventListener('click', function() { closeModal('qr-show-modal'); });
  document.getElementById('repair-close').addEventListener('click', function() { closeModal('repair-modal'); });
  document.getElementById('qr-show-modal').addEventListener('click', function(e) { if (e.target === this) closeModal('qr-show-modal'); });
  document.getElementById('qr-scan-modal').addEventListener('click', function(e) { if (e.target === this) { stopScanStream(); closeModal('qr-scan-modal'); } });

  document.getElementById('btn-show-recovery-qr').addEventListener('click', async () => {
    if (!activeWallet) return;
    var statusEl = document.getElementById('qr-show-status');
    var wrapEl = document.getElementById('qr-canvas-wrap');
    statusEl.innerHTML = '<div class="modal-loading">Loading...</div>';
    wrapEl.classList.add('hidden');
    openModal('qr-show-modal');

    // Unlock if needed
    if (walletEncrypted && !walletUnlocked) {
      try { await requireUnlock(); } catch(e) { closeModal('qr-show-modal'); return; }
    }

    try {
      var r = await rpc('getwalletsecret', []);
      if (r.error) throw new Error(r.error);
      var mnemonic = r.result.mnemonic;
      if (!mnemonic) {
        statusEl.innerHTML = '<div class="error-msg">Could not retrieve recovery phrase. Make sure the wallet is unlocked.</div>';
        return;
      }
      statusEl.innerHTML = '';
      wrapEl.classList.remove('hidden');
      var canvas = document.getElementById('qr-canvas');
      QR.render(canvas, mnemonic, 4);
    } catch(e) {
      statusEl.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  });

  // QR Scanner
  var _scanStream = null;

  function stopScanStream() {
    if (_scanStream) {
      _scanStream.getTracks().forEach(function(t) { t.stop(); });
      _scanStream = null;
    }
    var video = document.getElementById('qr-video');
    video.srcObject = null;
  }

  document.getElementById('qr-scan-close').addEventListener('click', function() {
    stopScanStream();
    closeModal('qr-scan-modal');
  });

  function fillImportGrid(text) {
    var words = text.trim().split(/\s+/);
    var inputs = document.querySelectorAll('#import-grid input');
    inputs.forEach(function(inp, i) { inp.value = words[i] || ''; });
  }

  document.getElementById('btn-scan-qr').addEventListener('click', async function() {
    // iOS: use native AVFoundation QR scanner via Tauri command
    if (window.fistbump.platform === 'ios') {
      try {
        var result = await __invoke('scan_qr');
        if (result) fillImportGrid(result);
      } catch(e) {
        // cancelled or not available — ignore
      }
      return;
    }

    // Android + Desktop: use camera + BarcodeDetector/getUserMedia
    var statusEl = document.getElementById('qr-scan-status');
    var video = document.getElementById('qr-video');
    video.classList.add('hidden');
    statusEl.innerHTML = '<div class="modal-loading">Starting camera...</div>';
    openModal('qr-scan-modal');

    var detector = null;
    try {
      if (typeof BarcodeDetector !== 'undefined') {
        detector = new BarcodeDetector({ formats: ['qr_code'] });
      }
    } catch(e) {}

    if (!detector) {
      statusEl.innerHTML = '<div class="error-msg">QR scanning is not supported on this device. Please type or paste the recovery phrase instead.</div>';
      return;
    }

    try {
      var stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' } });
      _scanStream = stream;
      video.srcObject = stream;
      await video.play();
      video.classList.remove('hidden');
      statusEl.innerHTML = '<div class="muted text-md">Point your camera at a recovery QR code</div>';

      var scanning = true;
      var scanCanvas = document.createElement('canvas');
      var scanCtx = scanCanvas.getContext('2d');
      var scanCount = 0;
      // QR scan loop
      function scanFrame() {
        if (!scanning || !_scanStream) return;
        if (video.readyState < 2) { setTimeout(scanFrame, 200); return; }
        scanCanvas.width = video.videoWidth;
        scanCanvas.height = video.videoHeight;
        scanCtx.drawImage(video, 0, 0);
        scanCount++;
        detector.detect(scanCanvas).then(function(codes) {
          if (!scanning || !_scanStream) return;
          if (codes.length) {
            if (codes[0].rawValue) {
              scanning = false;
              stopScanStream();
              video.style.display = 'none';
              closeModal('qr-scan-modal');
              fillImportGrid(codes[0].rawValue);
              return;
            }
          }
          setTimeout(scanFrame, 300);
        }).catch(function(e) {
          if (scanning) setTimeout(scanFrame, 500);
        });
      }
      scanFrame();

    } catch(e) {
      statusEl.innerHTML = '<div class="error-msg">Could not access camera: ' + esc(e.message) + '</div>';
    }
  });

  document.getElementById('btn-rescan').addEventListener('click', async () => {
    const el = document.getElementById('rescan-status');
    if (!await showConfirm('Rescan the blockchain for this wallet? This may take a while.')) return;
    el.innerHTML = '<div class="modal-loading">Starting rescan...</div>';
    try {
      const res = await rpc('rescanwallet', [0]);
      if (res.error) throw new Error(res.error);
      el.innerHTML = '<div class="modal-loading">Rescan started...</div>';
      // Poll progress
      const poll = setInterval(async () => {
        try {
          const p = await rpc('getrescanprogress');
          if (p.error || !p.result) {
            clearInterval(poll);
            el.innerHTML = '<div class="success-msg">Rescan complete.</div>';
            setTimeout(function() { el.innerHTML = ''; }, 3000);
            refresh();
            loadWalletInfo();
            return;
          }
          const r = p.result;
          const pct = r.percent != null ? r.percent.toFixed(1) : '0.0';
          const current = r.current != null ? r.current.toLocaleString() : '?';
          const total = r.total != null ? r.total.toLocaleString() : '?';
          el.innerHTML = '<div class="modal-loading">Rescanning... ' + pct + '% (' + current + ' / ' + total + ' blocks)</div>';
        } catch(e) {}
      }, 2000);
    } catch(e) {
      el.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  });

  document.getElementById('btn-delete-wallet').addEventListener('click', async () => {
    if (!activeWallet) return;
    if (!await requireUnlock()) return;
    if (!await showConfirm('Permanently delete wallet "' + activeWallet + '"? This cannot be undone.', { danger: true, okText: 'Delete' })) return;
    if (!await showConfirm('Are you sure? All wallet data will be lost.', { danger: true, okText: 'Delete' })) return;
    try {
      const res = await rpc('deletewallet');
      if (res.error) throw new Error(res.error);
      // Clean up biometric data
      if (activeWallet) {
        localStorage.removeItem('biometric_' + activeWallet);
        try { await __invoke('biometric_delete', { wallet: activeWallet }); } catch(e) {}
      }
      activeWallet = null;
      currentAddress = null;
      // Fade out app, prepare login, fade in login — same as doLogout
      await doLogout();
    } catch(e) {
      showAlert(friendlyError(e.message));
    }
  });

  // ---- Wallet Encryption ----

  document.getElementById('btn-encrypt-wallet').addEventListener('click', async () => {
    var el = document.getElementById('encrypt-status');
    var pass1 = await showPassphrasePrompt('Enter a passphrase to encrypt your wallet:');
    if (!pass1) return;
    var pass2 = await showPassphrasePrompt('Confirm passphrase:');
    if (!pass2) return;
    if (pass1 !== pass2) {
      el.innerHTML = '<div class="error-msg">Passphrases do not match.</div>';
      return;
    }
    if (!await showConfirm(
      'You will need this passphrase to send transactions. Make sure you have a backup of your mnemonic first.',
      { title: 'Encrypt wallet?', okText: 'Encrypt' }
    )) return;
    el.innerHTML = '<div class="modal-loading">Encrypting...</div>';
    try {
      var res = await rpc('encryptwallet', [pass1]);
      if (res.error) throw new Error(res.error);
      walletEncrypted = true;
      walletUnlocked = false;
      updateLockIndicator();
      el.innerHTML = '<div class="success-msg">Wallet encrypted.</div>';
    } catch(e) {
      el.innerHTML = '<div class="error-msg">' + esc(friendlyError(e.message)) + '</div>';
    }
  });

  document.getElementById('btn-lock-wallet').addEventListener('click', async () => {
    if (!walletEncrypted) return;
    if (walletUnlocked) {
      // Lock
      try {
        var res = await rpc('walletlock');
        if (res.error) throw new Error(res.error);
        walletUnlocked = false;
        updateLockIndicator();
      } catch(e) {
        showAlert(friendlyError(e.message));
      }
    } else {
      // Unlock
      requireUnlock();
    }
  });

  // ---- Theme ----

  function applyTheme(theme) {
    let isLight;
    if (theme === 'system') {
      isLight = !window.matchMedia('(prefers-color-scheme: dark)').matches;
    } else {
      isLight = theme === 'light';
    }
    document.body.classList.toggle('light', isLight);
    window.FistbumpBars?.setLightMode(isLight);
  }

  const themeSelect = document.getElementById('setting-theme');
  themeSelect.value = localStorage.getItem('theme') || 'system';
  applyTheme(themeSelect.value);

  themeSelect.addEventListener('change', () => {
    localStorage.setItem('theme', themeSelect.value);
    applyTheme(themeSelect.value);
  });

  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
    if ((localStorage.getItem('theme') || 'system') === 'system') {
      applyTheme('system');
    }
  });

  // ---- Mining ----

  const minerInput = document.getElementById('setting-miner-address');
  const minerSaveBtn = document.getElementById('btn-save-miner');
  const minerStatus = document.getElementById('miner-status');
  const miningSwitch = document.getElementById('mining-switch');
  const settingsMiningSwitch = document.getElementById('setting-mining-switch');

  function updateMiningToggleUI(enabled) {
    miningSwitch.checked = enabled;
    settingsMiningSwitch.checked = enabled;
  }

  // Load current settings
  (async function() {
    try {
      const s = await __invoke('get_settings');
      if (s.minerAddress) minerInput.value = s.minerAddress;
      updateMiningToggleUI(s.miningEnabled);
    } catch(e) {}
  })();

  var miningToggling = false;

  async function handleMiningToggle(enabled) {
    if (window.__APP_STORE__ === true) {
      showToast('In accordance with App Store policies, mining is not available.');
      updateMiningToggleUI(false);
      return;
    }
    if (miningToggling) {
      updateMiningToggleUI(!enabled);
      return;
    }
    miningToggling = true;
    miningSwitch.disabled = true;
    settingsMiningSwitch.disabled = true;
    document.getElementById('status-dot').className = 'status-dot connecting';
    document.getElementById('sync-status').textContent = 'Restarting...';
    // Allow DOM to repaint before blocking on toggle_mining
    await new Promise(r => setTimeout(r, 50));
    if (enabled) {
      try {
        const s = await __invoke('get_settings');
        if (!s.minerAddress) {
          updateMiningToggleUI(false);
          miningToggling = false;
          miningSwitch.disabled = false;
          settingsMiningSwitch.disabled = false;
          showToast('Set a miner address in Settings first');
          return;
        }
      } catch(e) {}
    }
    try {
      await __invoke('toggle_mining', { enabled });
      updateMiningToggleUI(enabled);
      // Wait for the node to finish restarting by watching for "P2P listening" in logs
      // (logs are cleared on restart so only new lines are present)
      await new Promise(function(resolve) {
        var timeout = setTimeout(resolve, 30000); // 30s max wait
        var check = setInterval(async function() {
          try {
            var lines = await __invoke('get_log');
            for (var i = lines.length - 1; i >= Math.max(0, lines.length - 20); i--) {
              if (lines[i] && lines[i].indexOf('P2P listening') >= 0) {
                clearInterval(check);
                clearTimeout(timeout);
                resolve();
                return;
              }
            }
          } catch(e) {}
        }, 500);
      });

      // Reconnect SSE and refresh state after restart
      connectEventStream();
    } catch(e) {
      updateMiningToggleUI(!enabled);
    }
    miningToggling = false;
    miningSwitch.disabled = false;
    settingsMiningSwitch.disabled = false;
  }

  miningSwitch.addEventListener('change', function() {
    if (!miningToggling) handleMiningToggle(miningSwitch.checked);
  });
  settingsMiningSwitch.addEventListener('change', function() {
    if (!miningToggling) handleMiningToggle(settingsMiningSwitch.checked);
  });

  minerSaveBtn.addEventListener('click', async function() {
    if (window.__APP_STORE__ === true) {
      showToast('In accordance with App Store policies, mining is not available.');
      return;
    }
    minerSaveBtn.style.opacity = '0.5';
    minerSaveBtn.style.pointerEvents = 'none';
    minerStatus.textContent = 'Saving...';
    try {
      await __invoke('set_miner_address', { address: minerInput.value, threads: parseInt(threadSlider.value) || 0 });
      minerStatus.textContent = 'Saved. Re-enable mining to apply changes.';
      setTimeout(() => { minerStatus.textContent = ''; }, 5000);
    } catch(e) {
      minerStatus.textContent = 'Error: ' + friendlyError(e.message || e);
    }
    minerSaveBtn.style.opacity = '';
    minerSaveBtn.style.pointerEvents = '';
  });

  // Miner threads slider
  var threadSlider = document.getElementById('setting-miner-threads');
  function updateSliderFill(slider) {
    var pct = (slider.value - slider.min) / (slider.max - slider.min) * 100;
    slider.style.setProperty('--range-fill', pct + '%');
  }
  threadSlider.addEventListener('input', function() {
    var cpus = parseInt(threadSlider.max) + 1;
    document.getElementById('miner-thread-label').textContent = threadSlider.value + ' / ' + cpus;
    updateSliderFill(threadSlider);
  });

  // ---- Proxy Setup ----

  function updateSetupChecks(status) {
    var caCheck = document.getElementById('setup-check-ca');
    var pacCheck = document.getElementById('setup-check-pac');
    if (caCheck) caCheck.className = 'setup-check' + (status.caTrusted ? ' done' : '');
    if (pacCheck) pacCheck.className = 'setup-check' + (status.pacInstalled ? ' done' : '');
    // Also update settings page if visible
    var settingsCa = document.getElementById('settings-ca-status');
    var settingsPac = document.getElementById('settings-pac-status');
    if (settingsCa) settingsCa.innerHTML = status.caTrusted
      ? '<span class="dot ok"></span>Installed' : '<span class="dot err"></span>Not installed';
    if (settingsPac) settingsPac.innerHTML = status.pacInstalled
      ? '<span class="dot ok"></span>Active' : '<span class="dot err"></span>Not active';
    var expiresEl = document.getElementById('settings-ca-expires');
    if (expiresEl) expiresEl.textContent = status.caExpires || '--';
    var proxyToggle = document.getElementById('setting-proxy-toggle');
    if (proxyToggle) proxyToggle.checked = !!status.proxyEnabled;
  }

  async function doSetupProxy(btn, statusEl) {
    if (statusEl) statusEl.textContent = 'Setting up... You may be prompted for your password.';
    if (btn) { btn.style.opacity = '0.5'; btn.style.pointerEvents = 'none'; }
    try {
      var result = await __invoke('setup_proxy');
      // Re-fetch full status to get caExpires
      var status = await __invoke('get_proxy_status');
      updateSetupChecks(status);
      if (statusEl) statusEl.textContent = result.setupComplete ? '' : 'Setup incomplete. Please try again.';
      return result.setupComplete;
    } catch(e) {
      if (statusEl) statusEl.textContent = 'Error: ' + friendlyError(e.message || e);
      return false;
    } finally {
      if (btn) { btn.style.opacity = ''; btn.style.pointerEvents = ''; }
    }
  }

  async function showSetup() {
    var setupScreen = document.getElementById('setup-screen');
    var loginScreen = document.getElementById('login-screen');
    if (!setupScreen) { loginScreen.classList.remove('hidden'); initLogin(); return; }

    // On mobile, skip
    if (fistbump.platform === 'ios' || fistbump.mobile) {
      loginScreen.classList.remove('hidden');
      initLogin();
      return;
    }

    var status;
    try {
      status = await __invoke('get_proxy_status');
    } catch(e) {
      loginScreen.classList.remove('hidden');
      initLogin();
      return;
    }

    updateSetupChecks(status);

    // Only show setup wizard on first launch (setupDone flag not set)
    if (status.setupDone || status.setupComplete) {
      loginScreen.classList.remove('hidden');
      initLogin();
      return;
    }
    setupScreen.classList.remove('hidden');
    loginScreen.classList.add('hidden');

    document.getElementById('btn-setup-install').onclick = async function() {
      var ok = await doSetupProxy(this, document.getElementById('setup-status'));
      if (ok) {
        setupScreen.classList.add('hidden');
        loginScreen.classList.remove('hidden');
        initLogin();
      }
    };

    document.getElementById('btn-setup-skip').onclick = function() {
      setupScreen.classList.add('hidden');
      loginScreen.classList.remove('hidden');
      initLogin();
    };
  }

  // Settings page: proxy toggle
  var proxyToggle = document.getElementById('setting-proxy-toggle');
  if (proxyToggle) {
    proxyToggle.addEventListener('change', async function() {
      var statusEl = document.getElementById('settings-proxy-status');
      proxyToggle.disabled = true;
      if (statusEl) statusEl.textContent = this.checked ? 'Enabling... You may be prompted for your password.' : 'Disabling...';
      try {
        await __invoke('toggle_proxy', { enabled: this.checked });
        var status = await __invoke('get_proxy_status');
        updateSetupChecks(status);
        if (statusEl) statusEl.textContent = '';
      } catch(e) {
        if (statusEl) statusEl.textContent = 'Error: ' + (e.message || e);
        this.checked = !this.checked;
      }
      proxyToggle.disabled = false;
    });
  }

  // Settings page: renew CA button
  var renewCaBtn = document.getElementById('btn-settings-renew-ca');
  if (renewCaBtn) {
    renewCaBtn.addEventListener('click', async function() {
      var statusEl = document.getElementById('settings-proxy-status');
      renewCaBtn.style.opacity = '0.5';
      renewCaBtn.style.pointerEvents = 'none';
      if (statusEl) statusEl.textContent = 'Renewing certificate... You may be prompted for your password.';
      try {
        await __invoke('renew_ca');
        if (statusEl) {
          statusEl.textContent = 'Certificate renewed.';
          setTimeout(function() { statusEl.textContent = ''; }, 3000);
        }
        var status = await __invoke('get_proxy_status');
        updateSetupChecks(status);
      } catch(e) {
        if (statusEl) statusEl.textContent = 'Failed: ' + (e.message || e);
      }
      renewCaBtn.style.opacity = '';
      renewCaBtn.style.pointerEvents = '';
    });
  }

  // Refresh proxy status when settings tab is shown
  document.querySelectorAll('.nav-item[data-tab="settings"]').forEach(function(el) {
    el.addEventListener('click', async function() {
      try {
        var status = await __invoke('get_proxy_status');
        updateSetupChecks(status);
      } catch(e) {}
    });
  });

  // ---- Init ----

  // Desktop only: if the Rust side detected a legacy ~/.fbd install with
  // existing wallets, offer to copy them over before fbd starts. The backend
  // has deferred start_node() until resolve_migration() is called, so the
  // normal "connecting to node..." retry loop in initLogin will happily wait.
  (async function() {
    var pending = null;
    try {
      pending = await __invoke('get_pending_migration');
    } catch(e) {
      // Command missing on mobile or pre-migration builds — just proceed.
    }
    if (pending) {
      var plural = pending.wallet_count === 1 ? 'wallet' : 'wallets';
      var msg = 'Found ' + pending.wallet_count + ' existing ' + plural +
                ' from an older fbd install at:\n\n' + pending.source +
                '\n\nCopy ' + (pending.wallet_count === 1 ? 'it' : 'them') +
                ' into this wallet?';
      var accept = await showConfirm(msg, { okText: 'Copy' });
      try {
        await __invoke('resolve_migration', { accept: accept });
      } catch(e) {
        await showAlert('Migration failed: ' + (e && e.message ? e.message : e) +
                        '\n\nThe wallet will start without copying.');
        // Best-effort: try to proceed anyway so the app isn't permanently stuck.
        try { await __invoke('resolve_migration', { accept: false }); } catch(e2) {}
      }
    }
    showSetup();
  })();

  // SSE event stream for live updates from the node
  var _es = null;
  var _esConnected = false;
  var _esPeerCount = -1; // -1 = unknown
  var _esLastHeight = 0;
  var _esLastProgress = 1.0;

  function estimateBlockDate(blockHeight) {
    if (!_esLastHeight || blockHeight <= 0) return null;
    var secondsAgo = (_esLastHeight - blockHeight) * 60;
    var d = new Date(Date.now() - secondsAgo * 1000);
    var time = d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
    var date = d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
    return time + '<div class="muted text-sm">' + date + '</div>';
  }

  function updateStatusBar(height, progress, peerCount) {
    const dot = document.getElementById('status-dot');
    const label = document.getElementById('sync-status');
    document.getElementById('about-height').innerHTML =
      '<a href="' + EXPLORER + '/block/' + height + '" target="_blank">' + height.toLocaleString() + '</a>';
    if (peerCount >= 0) document.getElementById('about-peers').textContent = peerCount;

    if (peerCount === 0) {
      dot.className = 'status-dot error';
      label.textContent = 'No peers';
    } else if (progress < 0.999) {
      dot.className = 'status-dot syncing';
      label.textContent = 'Syncing ' + (progress * 100).toFixed(1) + '%';
    } else {
      dot.className = 'status-dot connected';
      label.textContent = 'Block ' + height.toLocaleString();
    }
  }

  var _lastBalance = null;

  function updateBalance(b) {
    _lastBalance = b;
    document.getElementById('balance').innerHTML =
      formatFBC(b.spendable);
    var items = '';
    // Only show confirmed if it differs from spendable (otherwise redundant)
    if (b.confirmed !== b.spendable) {
      items += '<div class="bal-item"><div class="bal-label">Confirmed</div><div class="bal-value">' + formatFBC(b.confirmed) + '</div></div>';
    }
    if (b.pending) {
      items += '<div class="bal-item"><div class="bal-label">Pending</div><div class="bal-value">' + (b.pending > 0 ? '+' : '') + formatFBC(b.pending) + '</div></div>';

    }
    if (b.locked) {
      items += '<div class="bal-item"><div class="bal-label">Locked</div><div class="bal-value">' + formatFBC(b.locked) + '</div></div>';
    }
    if (b.immature) {
      items += '<div class="bal-item"><div class="bal-label">Immature</div><div class="bal-value">' + formatFBC(b.immature) + '</div></div>';
    }
    if (b.forfeited) {
      items += '<div class="bal-item"><div class="bal-label">Forfeited</div><div class="bal-value text-red">' + formatFBC(b.forfeited) + '</div></div>';
    }
    document.getElementById('balance-details').innerHTML = '<div class="balance-grid">' + items + '</div>';
  }

  async function connectEventStream() {
    if (_es) { try { _es.close(); } catch(e) {} }
    var es;
    var apiKey = '';
    try { apiKey = await window.__TAURI__.core.invoke('get_api_key_cmd') || ''; } catch(e) {}
    var params = [];
    if (activeWallet) params.push('wallet=' + encodeURIComponent(activeWallet));
    if (apiKey) params.push('key=' + encodeURIComponent(apiKey));
    var sseUrl = 'http://127.0.0.1:' + NETWORKS[network].rpcPort + '/events' + (params.length ? '?' + params.join('&') : '');
    try { es = new EventSource(sseUrl); } catch(e) { return; }
    _es = es;

    es.onopen = function() {
      _esConnected = true;
      var dot = document.getElementById('status-dot');
      var label = document.getElementById('sync-status');
      dot.className = 'status-dot connected';
      label.textContent = 'Connected';
      // Refresh state immediately after reconnect so peer count / height are current
      refresh();
      // Re-fetch full log to pick up lines emitted during node restart
      refreshLog();
    };

    es.onmessage = function(ev) {
      try {
        var msg = JSON.parse(ev.data);
        if (msg.type === 'block') {
          _esPeerCount = msg.peers;
          _esLastHeight = msg.height;
          _esLastProgress = msg.progress;
          _blockToast = true;
          updateStatusBar(msg.height, msg.progress, msg.peers);
          // Refresh name detail page on every block (auction state may change)
          if (currentNameDetail && !document.activeElement.matches('input, textarea, select')) {
            openNameDetail(currentNameDetail);
          }
        } else if (msg.type === 'peers') {
          _esPeerCount = msg.count;
          updateStatusBar(_esLastHeight, _esLastProgress, msg.count);
        } else if (msg.type === 'wallet') {
          if (msg.balance && activeWallet) updateBalance(msg.balance);
          if (activeWallet) {
            refreshTxList();
            if (_esLastProgress >= 0.999) refreshOverviewActions();
            // Refresh name detail page (wallet event fires after block indexing, so data is fresh)
            if (currentNameDetail && !document.activeElement.matches('input, textarea, select')) {
              openNameDetail(currentNameDetail);
            }
          }
        } else if (msg.type === 'mined') {
          showToast('Block mined! +' + formatFBC(msg.reward));
        } else if (msg.type === 'log') {
          logRaw.push(msg.line);
          if (logRaw.length > 500) logRaw.splice(0, logRaw.length - 500);
          if (document.getElementById('tab-log').classList.contains('active')) renderLog();
        }
      } catch(e) {}
    };

    es.onerror = function(e) {
      // Close the old EventSource to prevent duplicate auto-reconnects
      try { es.close(); } catch(ex) {}
      _es = null;
      _esConnected = false;
      var dot = document.getElementById('status-dot');
      var label = document.getElementById('sync-status');
      dot.className = 'status-dot connecting';
      label.textContent = 'Connecting...';
      setTimeout(connectEventStream, 3000);
    };
  }

  // Load network from backend (set via tauri.conf.json at build time), then connect SSE
  __invoke('get_settings').then(function(s) {
    if (s.network && NETWORKS[s.network]) network = s.network;
  }).catch(function() {}).finally(function() {
    connectEventStream();
  });
  // Fallback poll for initial state and when SSE is disconnected
  setInterval(() => { if (activeWallet && !_esConnected) refresh(); }, 10000);

  // ── Foreground recovery ──────────────────────────────────────────
  //
  // Mobile is brutal on a long-lived node: iOS suspends us after ~30s
  // in the background (severing TCP sockets), and Android can kill the
  // fbd child outright to reclaim memory. The old "just call
  // rpc('reconnect')" path fell over in practice because:
  //   1. If fbd is actually dead, the RPC silently times out and
  //      nothing gets restarted.
  //   2. The EventSource may report itself as still connected from the
  //      browser's side even though the underlying pipe is dead —
  //      onerror doesn't fire until the next attempted read.
  //
  // New policy: track how long we've been hidden. On a short flip
  // (<5s, e.g. pulling down Control Centre) we just nudge fbd and
  // refresh. On anything longer we invoke the `restart_node` Tauri
  // command — which on desktop/Android kills and respawns the fbd
  // child process, and on iOS triggers FBDNode.shared.restart() via
  // FFI — and force an SSE reconnect so the browser doesn't sit on a
  // stale EventSource. The 3s retry loop in connectEventStream's
  // onerror handles waiting for the new fbd to be ready.
  var _lastHiddenAt = 0;
  var FOREGROUND_RESTART_THRESHOLD_MS = 5000;

  document.addEventListener('visibilitychange', async function() {
    if (document.visibilityState === 'hidden') {
      _lastHiddenAt = Date.now();
      return;
    }

    // We're coming back to foreground.
    var hiddenMs = _lastHiddenAt ? Date.now() - _lastHiddenAt : 0;
    var longBackground = hiddenMs >= FOREGROUND_RESTART_THRESHOLD_MS;

    if (longBackground) {
      // Full tear-down + restart on the native side. Errors are
      // swallowed — if the restart fails, connectEventStream's retry
      // loop will keep trying to reconnect anyway.
      try {
        await __invoke('restart_node');
      } catch (e) {}
    }

    // Always force an SSE reconnect: the existing EventSource may
    // believe it's still open even though the underlying socket was
    // torn down by the OS. connectEventStream() closes the old _es
    // before opening a new one.
    _esConnected = false;
    connectEventStream();

    // Fall through to the peer-reconnect nudge + UI refresh — cheap,
    // and useful even on short backgroundings where fbd is still fine.
    rpc('reconnect').catch(function() {});
    if (activeWallet) refresh();
  });

  // ---- Demo Mode ----
  // Populates the overview with fake data for screenshots.
  // Trigger: type "demo" on the keyboard while on the overview page.

  var _demoBuffer = '';
  document.addEventListener('keydown', function(e) {
    if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.tagName === 'SELECT') return;
    _demoBuffer += e.key.toLowerCase();
    if (_demoBuffer.length > 4) _demoBuffer = _demoBuffer.slice(-4);
    if (_demoBuffer === 'demo') {
      _demoBuffer = '';
      activateDemo();
    }
  });

  function activateDemo() {
    // Show app, hide login
    document.getElementById('login-screen').classList.add('hidden');
    document.getElementById('setup-screen').classList.add('hidden');
    document.getElementById('app').classList.remove('hidden');

    // Status bar
    var height = 1337;
    document.getElementById('status-dot').className = 'status-dot connected';
    document.getElementById('sync-status').textContent = 'Block ' + height.toLocaleString();
    document.getElementById('about-height').textContent = height.toLocaleString();
    document.getElementById('about-peers').textContent = '12';
    document.getElementById('about-network').textContent = 'mainnet';
    document.getElementById('about-node').textContent = 'fbd 1.0.0';

    // Balance — spendable spells FISTBUMP on phone keyboard (34782867)
    var spendable = 34782867000;
    var confirmed = 39282867000;
    var unconfirmed = confirmed + 1500000000;
    var locked = 4500000000;
    document.getElementById('balance').innerHTML =
      formatFBC(spendable);
    var details = '<span>Confirmed: ' + formatFBC(confirmed) + '</span>';
    details += '<span>Unconfirmed: +' + formatFBC(unconfirmed - confirmed) + '</span>';
    details += '<span>Locked: ' + formatFBC(locked) + '</span>';
    document.getElementById('balance-details').innerHTML = details;

    // Fake transactions using the real renderTxItem format
    var now = Math.floor(Date.now() / 1000);
    function randHash() {
      var h = '';
      for (var i = 0; i < 64; i++) h += '0123456789abcdef'[Math.floor(Math.random() * 16)];
      return h;
    }
    var txs = [
      { net:  75000000000, timestamp: now - 180,    covenants: ['REVEAL eskimo'],     type: 'send' },
      { net: -50000000000, timestamp: now - 3800,   covenants: ['BID fbd'],           type: 'send' },
      { net:      -3042,   timestamp: now - 7200,   covenants: ['OPEN fbd'],          type: 'send' },
      { net:  -7500000000, timestamp: now - 14400,  covenants: [],                    type: 'send' },
      { net:   2000000000, timestamp: now - 43200,  covenants: [],                    type: 'receive' },
      { net: -1000000000000, timestamp: now - 86400,  covenants: ['REGISTER fistbump'], type: 'send' },
      { net:    500000000, timestamp: now - 259200, covenants: [],                    coinbase: true },
    ];
    txs.forEach(function(tx) { tx.txid = randHash(); });
    var el = document.getElementById('tx-list');
    el.className = '';
    el.innerHTML = txs.map(renderTxItem).join('');

    // Wallet name
    document.getElementById('sidebar-wallet-name').textContent = 'demo';

    // Mining toggle — visually on without actually enabling
    document.getElementById('mining-switch').checked = true;
    document.getElementById('setting-mining-switch').checked = true;

    // Hide action alerts
    document.getElementById('overview-actions').classList.add('hidden');
    document.getElementById('overview-actions').innerHTML = '';

    // Switch to overview
    document.querySelectorAll('.page').forEach(function(p) { p.classList.remove('active'); });
    document.getElementById('tab-overview').classList.add('active');
    document.querySelectorAll('.nav-item').forEach(function(n) { n.classList.remove('active'); });
    var overviewNav = document.querySelector('.nav-item[data-tab="overview"]');
    if (overviewNav) overviewNav.classList.add('active');
  }
})();

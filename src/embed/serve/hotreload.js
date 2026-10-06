// ==========================================================================
// Tola Hot Reload Runtime (Anchor-based)
// ==========================================================================
//
// All operations use StableId (data-tola-id) for targeting.
// No position indices - uses anchor-based insertion instead.
//
// This design ensures:
// - Order independence (operations can execute in any order)
// - No index drift bugs
// - Simple, predictable behavior

(function() {
  const ERROR_OVERLAY_CSS = `__TOLA_ERROR_OVERLAY_CSS__`;

  const Tola = {
    // StableId -> Element mapping for O(1) lookups
    idMap: new Map(),
    errorState: new Map(),
    ws: null,
    wsPort: null,
    reconnectTimer: null,
    reconnectRetries: 0,
    maxReconnectRetries: 30,
    pageActive: true,
    suppressNextClose: false,
    suppressReloadUntil: 0,
    reconnectDelay: 1000,
    pendingReload: null,
    pendingReloadTimer: null,
    reloadInProgress: false,
    messageQueue: Promise.resolve(),

    closeWsSilently() {
      if (!this.ws) return;
      this.suppressNextClose = true;
      try {
        this.ws.close();
      } catch (_) {}
    },

    // Hydrate: build idMap from existing DOM
    hydrate() {
      this.idMap.clear();
      document.querySelectorAll('[data-tola-id]').forEach(el => {
        this.idMap.set(el.dataset.tolaId, el);
      });
      console.log('[tola] hydrated', this.idMap.size, 'nodes');
    },

    // Connect to WebSocket server
    connect(port) {
      if (typeof port === 'number') {
        this.wsPort = port;
      }
      if (!this.wsPort) return;

      // Avoid opening duplicate sockets while reconnecting.
      if (this.ws && (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING)) {
        return;
      }

      const wsScheme = window.location.protocol === 'https:' ? 'wss' : 'ws';
      const wsHost = window.location.hostname || 'localhost';
      const ws = new WebSocket(`${wsScheme}://${wsHost}:${this.wsPort}/`);
      this.ws = ws;

      ws.onopen = () => {
        console.log('[tola] hot reload connected');
        this.reconnectDelay = 1000;
        this.reconnectRetries = 0;
        if (this.reconnectTimer) {
          clearTimeout(this.reconnectTimer);
          this.reconnectTimer = null;
        }
        this.hydrate();
        // Report current page to the server so later hot-reload work can
        // prioritize the page the browser is currently showing.
        this.reportCurrentPage();
      };

      ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          this.enqueueMessage(msg);
        } catch (err) {
          console.error('[tola] message error:', err);
        }
      };

      ws.onclose = () => {
        // Ignore stale sockets replaced by a newer reconnect attempt.
        if (this.ws !== ws) return;
        this.ws = null;

        // Navigation/back-forward cache lifecycle can close sockets normally.
        // Skip reconnect in that case; pageshow/visible will trigger reconnect.
        if (this.suppressNextClose || !this.pageActive) {
          this.suppressNextClose = false;
          return;
        }

        console.log('[tola] disconnected, attempting reconnect...');
        this.attemptReconnect();
      };

      // Keep onerror silent to reduce console noise; onclose drives reconnect.
      ws.onerror = () => {};
    },

    // Attempt to reconnect with exponential backoff.
    // Do not auto-reload the page on transient disconnects (e.g. laptop sleep).
    attemptReconnect() {
      if (!this.wsPort) return;
      if (this.reconnectTimer) return;
      if (!this.pageActive) return;
      if (this.ws && (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING)) {
        return;
      }
      if (document.visibilityState === 'hidden') {
        return;
      }
      if (navigator.onLine === false) {
        return;
      }
      if (this.reconnectRetries >= this.maxReconnectRetries) {
        console.log('[tola] giving up reconnect, please refresh manually');
        return;
      }

      const delay = this.reconnectRetries === 0
        ? 500
        : Math.min(1000 * Math.pow(1.3, this.reconnectRetries - 1), 5000);

      this.reconnectRetries += 1;
      console.log(`[tola] reconnect attempt ${this.reconnectRetries}/${this.maxReconnectRetries}`);
      this.reconnectTimer = setTimeout(() => {
        this.reconnectTimer = null;
        this.connect();
      }, delay);
    },

    setupReconnectTriggers() {
      window.addEventListener('popstate', () => {
        this.suppressReloadUntil = Date.now() + 1200;
      });

      window.addEventListener('pagehide', () => {
        this.pageActive = false;
        if (this.reconnectTimer) {
          clearTimeout(this.reconnectTimer);
          this.reconnectTimer = null;
        }

        // Proactively close to avoid dangling sockets during navigation.
        this.closeWsSilently();
      });

      // beforeunload covers cases where pagehide may not be dispatched first.
      window.addEventListener('beforeunload', () => {
        this.pageActive = false;
      });

      window.addEventListener('pageshow', (e) => {
        this.pageActive = true;
        // BFCache restore can deliver stale reload events briefly after resume.
        // Ignore reloads in a short grace window to prevent a flash.
        if (e && e.persisted) {
          this.suppressReloadUntil = Date.now() + 1200;
        }
        this.attemptReconnect();
        this.flushPendingReload();
      });

      document.addEventListener('visibilitychange', () => {
        if (document.visibilityState === 'visible') {
          this.pageActive = true;
          this.attemptReconnect();
          this.flushPendingReload();
        } else {
          // Keep the socket alive for ordinary tab switches so edits made while
          // you're in the editor still patch the page in the background.
          // Only close on real page lifecycle exits like pagehide/freeze.
          this.pageActive = true;
        }
      });

      // Freeze/resume lifecycle helps avoid suspension-time socket errors.
      document.addEventListener('freeze', () => {
        this.pageActive = false;
        this.closeWsSilently();
      });

      document.addEventListener('resume', () => {
        this.pageActive = true;
        this.attemptReconnect();
        this.flushPendingReload();
      });

      window.addEventListener('online', () => {
        this.pageActive = true;
        this.attemptReconnect();
        this.flushPendingReload();
      });
    },

    setupHistoryReloadGuard() {
      const getNavEntries = performance && performance.getEntriesByType;
      if (!getNavEntries) return;
      const entries = performance.getEntriesByType('navigation');
      if (!entries || entries.length === 0) return;

      const nav = entries[0];
      if (nav && nav.type === 'back_forward') {
        this.suppressReloadUntil = Date.now() + 1200;
      }
    },

    enqueueMessage(msg) {
      const run = () => Promise.resolve(this.handleMessage(msg)).catch((err) => {
        console.error('[tola] message error:', err);
      });
      this.messageQueue = this.messageQueue.then(run, run);
    },

    // Handle incoming message
    handleMessage(msg) {
      switch (msg.type) {
        case 'reload':
          this.handleReloadMessage(msg);
          return Promise.resolve();
        case 'patch':
          // StableIds are globally unique (include page path hash), so we can
          // safely apply all patches - only matching elements will be affected.
          // This naturally supports htmx/dynamic content loading.
          return this.applyPatches(msg.ops, msg.assets || [], msg.path).then(() => {
            // Clear SPA prefetch cache (content may have changed)
            if (window.TolaSpa && typeof window.TolaSpa.clearCaches === 'function') {
              window.TolaSpa.clearCaches();
            }

            // Seamless URL update when permalink changes (no reload)
            if (msg.url_change) {
              this.updateUrl(msg.url_change);
            }
          });
        case 'asset':
          return this.applyAssetMessage(msg);
        case 'ping':
          this.sendMessage({ type: 'pong', ts: msg.ts });
          return Promise.resolve();
        case 'pong':
          return Promise.resolve();
        case 'connected':
          console.log('[tola] server version:', msg.version);
          return Promise.resolve();
        case 'error':
          console.error('[tola] compile error:', msg.path, msg.error);
          this.errorState.set(msg.path, msg.error);
          this.renderErrorOverlay();
          return Promise.resolve();
        case 'clear_error':
          if (msg.path) {
            console.log('[tola] error cleared:', msg.path);
            this.errorState.delete(msg.path);
          } else {
            console.log('[tola] all errors cleared');
            this.errorState.clear();
          }
          this.renderErrorOverlay();
          return Promise.resolve();
        default:
          return Promise.resolve();
      }
    },

    handleReloadMessage(msg) {
      if (!this.canReloadNow()) {
        this.queueReload(msg);
        return;
      }

      this.reloadNow(msg);
    },

    canReloadNow() {
      return this.pageActive && document.visibilityState === 'visible';
    },

    queueReload(msg) {
      this.pendingReload = msg || { type: 'reload' };
    },

    flushPendingReload() {
      if (!this.pendingReload || !this.canReloadNow()) {
        return;
      }

      if (Date.now() < this.suppressReloadUntil) {
        this.schedulePendingReload();
        return;
      }

      const msg = this.pendingReload;
      this.pendingReload = null;
      this.reloadNow(msg);
    },

    schedulePendingReload() {
      if (this.pendingReloadTimer) {
        return;
      }

      const delay = Math.max(this.suppressReloadUntil - Date.now(), 0) + 10;
      this.pendingReloadTimer = setTimeout(() => {
        this.pendingReloadTimer = null;
        this.flushPendingReload();
      }, delay);
    },

    reloadNow(msg) {
      if (this.reloadInProgress) {
        return;
      }
      if (Date.now() < this.suppressReloadUntil) {
        this.queueReload(msg);
        this.schedulePendingReload();
        return;
      }

      this.reloadInProgress = true;
      console.log('[tola] reloading:', msg.reason || 'file changed');
      // If permalink changed, update URL before reload to avoid 404
      if (msg.url_change) {
        this.updateUrl(msg.url_change);
      }
      location.reload();
    },

    sendMessage(message) {
      if (this.ws && this.ws.readyState === WebSocket.OPEN) {
        this.ws.send(JSON.stringify(message));
      }
    },

    // Update browser URL bar without reload (seamless permalink change)
    updateUrl(urlChange) {
      // Decode URL for comparison (server sends decoded URLs)
      const currentPath = decodeURIComponent(window.location.pathname).replace(/\/$/, '') || '/';
      const oldPath = (urlChange.old || '').replace(/\/$/, '') || '/';
      if (currentPath === oldPath) {
        console.log('[tola] URL updated:', urlChange.old, '->', urlChange.new);

        // Migrate SPA scroll position before URL change
        if (window.TolaSpa && typeof window.TolaSpa.migrateScrollPosition === 'function') {
          window.TolaSpa.migrateScrollPosition(urlChange.old, urlChange.new);
        }

        history.pushState({ tola: true }, '', urlChange.new);
        // Report new route to server for targeted push
        this.reportCurrentPage();
      }
    },

    applyAssetMessage(msg) {
      if (!msg || typeof msg.href !== 'string' || msg.href.length === 0) {
        return Promise.resolve();
      }

      const link = this.findStylesheetLink(msg.href);
      if (link) {
        if (this.sameHref(link, msg.href)) {
          return Promise.resolve();
        }
        return this.seamlessCssUpdate(link, this.stylesheetHtml(link, msg.href));
      }

      if (this.isStylesheetHref(msg.href)) {
        return Promise.resolve();
      }

      this.handleReloadMessage({
        type: 'reload',
        reason: `asset changed: ${msg.href}`,
      });
      return Promise.resolve();
    },

    isStylesheetHref(href) {
      try {
        return new URL(href, window.location.href).pathname.toLowerCase().endsWith('.css');
      } catch (_) {
        return false;
      }
    },

    findStylesheetLink(href) {
      let target = null;
      try {
        target = new URL(href, window.location.href);
      } catch (_) {
        return null;
      }

      const candidates = [];
      for (const link of document.querySelectorAll('link[rel="stylesheet"]')) {
        try {
          const current = new URL(link.getAttribute('href') || link.href, window.location.href);
          if (current.origin === target.origin && current.pathname === target.pathname) {
            if (current.href === target.href) {
              return link;
            }
            if (!link.dataset.tolaPendingStylesheet) {
              candidates.push(link);
            }
          }
        } catch (_) {}
      }

      return candidates.length > 0 ? candidates[candidates.length - 1] : null;
    },

    sameHref(link, href) {
      try {
        const current = new URL(link.getAttribute('href') || link.href, window.location.href);
        const next = new URL(href, window.location.href);
        return current.href === next.href;
      } catch (_) {
        return false;
      }
    },

    stylesheetHtml(link, href) {
      const next = link.cloneNode(false);
      next.setAttribute('href', href);
      return next.outerHTML;
    },

    // Render error overlay from the current error set without reloading
    renderErrorOverlay() {
      const entries = Array.from(this.errorState.entries())
        .sort((a, b) => a[0].localeCompare(b[0]));

      if (entries.length === 0) {
        this.hideErrorOverlay();
        return;
      }

      const [path, error] = entries[0];
      const extraCount = entries.length - 1;
      let overlay = document.getElementById('tola-error-overlay');
      if (!overlay) {
        overlay = document.createElement('div');
        overlay.id = 'tola-error-overlay';
        overlay.innerHTML = `
          <style>${ERROR_OVERLAY_CSS}</style>
          <div class="tola-error-header">
            <span class="tola-error-title">Compilation Error</span>
            <button class="tola-error-close" onclick="Tola.hideErrorOverlay()">Dismiss</button>
          </div>
          <div class="tola-error-content">
            <div class="tola-error-path"></div>
            <div class="tola-error-message"></div>
          </div>
        `;
        document.body.appendChild(overlay);
      }

      const title = extraCount > 0
        ? `Compilation Errors (${entries.length})`
        : 'Compilation Error';
      const summary = extraCount > 0
        ? `${path} (+${extraCount} more)`
        : path;

      overlay.querySelector('.tola-error-title').textContent = title;
      overlay.querySelector('.tola-error-path').textContent = summary;
      // Use innerHTML since error contains HTML spans for syntax highlighting
      overlay.querySelector('.tola-error-message').innerHTML = error;
      overlay.style.display = 'flex';
    },

    // Hide error overlay
    hideErrorOverlay() {
      const overlay = document.getElementById('tola-error-overlay');
      if (overlay) overlay.style.display = 'none';
    },

    // Apply patch operations
    // Phase 1: apply stylesheet updates (replace/attrs) and wait for preload completion
    // Phase 2: apply all remaining DOM patches
    applyPatches(ops, assets, path) {
      ops = Array.isArray(ops) ? ops : [];
      assets = Array.isArray(assets) ? assets : [];

      const cssOps = [];
      const otherOps = [];
      const cssTasks = [];
      const deferredAssets = [];
      const patchOwnsPage = this.patchAppliesToCurrentPage(ops, path);

      for (const op of ops) {
        if (this.isStylesheetPatch(op)) {
          cssOps.push(op);
        } else {
          otherOps.push(op);
        }
      }

      const applyRemaining = (styleUpdates) => {
        for (const update of styleUpdates || []) {
          update.activate();
        }

        for (const op of otherOps) {
          try {
            this.applyPatch(op);
          } catch (err) {
            console.error('[tola] patch failed:', op.op, err);
            location.reload();
            return Promise.resolve();
          }
        }

        for (const update of styleUpdates || []) {
          update.cleanup();
        }
        this.hydrate();

        // Update recolor filter (CSS variables may have changed)
        if (window.TolaRecolor && typeof window.TolaRecolor.update === 'function') {
          window.TolaRecolor.update();
        }

        return this.applyDeferredAssets(deferredAssets);
      };

      for (const op of cssOps) {
        try {
          cssTasks.push(this.prepareStylesheetPatch(op));
        } catch (err) {
          console.error('[tola] css patch failed:', op.op, err);
          location.reload();
          return Promise.resolve();
        }
      }

      if (patchOwnsPage) {
        for (const href of assets) {
          const task = this.prepareAssetForPatch(href);
          if (task) {
            cssTasks.push(task);
          } else {
            deferredAssets.push(href);
          }
        }
      }

      if (cssTasks.length === 0) {
        return applyRemaining([]);
      }

      return Promise.all(cssTasks)
        .then(applyRemaining)
        .catch((err) => {
          console.error('[tola] css patch failed:', err);
          location.reload();
        });
    },

    patchAppliesToCurrentPage(ops, path) {
      return this.pathMatchesCurrentPage(path) || ops.some((op) => this.patchTargetExists(op));
    },

    pathMatchesCurrentPage(path) {
      const target = this.normalizeRoutePath(path);
      if (!target) return false;
      return target === this.normalizeRoutePath(window.location.pathname);
    },

    normalizeRoutePath(path) {
      if (typeof path !== 'string' || path.length === 0) return '';
      let value = path;
      try {
        value = decodeURIComponent(value);
      } catch (_) {}
      if (!value.startsWith('/')) value = `/${value}`;
      value = value.replace(/\/+$/, '');
      return value || '/';
    },

    patchTargetExists(op) {
      if (!op || typeof op.op !== 'string') return false;
      switch (op.op) {
        case 'replace':
        case 'text':
        case 'html':
        case 'remove':
        case 'attrs':
          return !!this.getById(op.target);
        case 'insert':
          return !!this.getById(op.anchor_id);
        case 'move':
          return !!(this.getById(op.target) && this.getById(op.anchor_id));
        default:
          return false;
      }
    },

    prepareAssetForPatch(href) {
      if (typeof href !== 'string' || href.length === 0) return null;
      const link = this.findStylesheetLink(href);
      if (!link) return null;
      if (this.sameHref(link, href)) return Promise.resolve(this.emptyStyleUpdate());
      return this.prepareStylesheetUpdate(link, this.stylesheetHtml(link, href));
    },

    applyDeferredAssets(assets) {
      let chain = Promise.resolve();
      for (const href of assets) {
        chain = chain.then(() => this.applyAssetMessage({ type: 'asset', href }));
      }
      return chain;
    },

    isStylesheetPatch(op) {
      return this.isStylesheetReplaceOp(op) || this.isStylesheetAttrsOp(op);
    },

    isStylesheetReplaceOp(op) {
      if (!op || op.op !== 'replace' || typeof op.html !== 'string') return false;
      const temp = document.createElement('div');
      temp.innerHTML = op.html;
      const link = temp.querySelector('link');
      return !!(link && link.rel === 'stylesheet');
    },

    isStylesheetAttrsOp(op) {
      if (!op || op.op !== 'attrs' || !Array.isArray(op.attrs) || !op.target) return false;
      const hasHrefUpdate = op.attrs.some(([name, value]) => name === 'href' && typeof value === 'string');
      if (!hasHrefUpdate) return false;
      const el = this.getById(op.target);
      return !!(el && el.tagName === 'LINK' && el.rel === 'stylesheet');
    },

    prepareStylesheetPatch(op) {
      if (!op) return Promise.resolve(this.emptyStyleUpdate());
      if (op.op === 'replace') return this.prepareStylesheetReplace(op);
      if (op.op === 'attrs') return this.prepareStylesheetAttrs(op);
      return Promise.resolve(this.emptyStyleUpdate());
    },

    prepareStylesheetReplace(op) {
      const el = this.getById(op.target);
      if (!el) {
        return Promise.resolve(this.emptyStyleUpdate());
      }
      if (el.tagName === 'LINK' && el.rel === 'stylesheet') {
        return this.prepareStylesheetUpdate(el, op.html);
      }
      // Fallback: if target exists but is not stylesheet, apply as normal replace
      return Promise.resolve(this.styleUpdate(() => this.applyPatch(op)));
    },

    prepareStylesheetAttrs(op) {
      const oldLink = this.getById(op.target);
      if (!(oldLink && oldLink.tagName === 'LINK' && oldLink.rel === 'stylesheet')) {
        return Promise.resolve(this.styleUpdate(() => this.applyPatch(op)));
      }

      const nextLink = oldLink.cloneNode(false);
      for (const [name, value] of op.attrs) {
        if (value === null) {
          nextLink.removeAttribute(name);
        } else {
          nextLink.setAttribute(name, value);
        }
      }

      const oldHref = oldLink.getAttribute('href') || '';
      const nextHref = nextLink.getAttribute('href') || '';
      const nextRel = (nextLink.getAttribute('rel') || '').toLowerCase();

      // Only use preload swap when stylesheet href actually changes.
      if (nextRel === 'stylesheet' && nextHref && nextHref !== oldHref) {
        return this.prepareStylesheetUpdate(oldLink, nextLink.outerHTML);
      }

      return Promise.resolve(this.styleUpdate(() => this.applyPatch(op)));
    },

    // Apply single patch - pure ID/anchor based, no position indices
    applyPatch(op) {
      switch (op.op) {
        case 'replace': {
          const el = this.getById(op.target);
          if (el) {
            // Seamless CSS update: preload new stylesheet before removing old one
            if (el.tagName === 'LINK' && el.rel === 'stylesheet') {
              this.seamlessCssUpdate(el, op.html);
            } else {
              this.morphOuterHtml(el, op.html);
            }
          }
          break;
        }

        case 'text': {
          // Update text content (for single-text-child elements)
          const el = this.getById(op.target);
          if (el) {
            el.textContent = op.text;
          } else {
            console.warn('[tola] text target not found:', op.target);
          }
          break;
        }

        case 'html': {
          // Replace inner HTML (for mixed content structure changes)
          const el = this.getById(op.target);
          if (el) {
            if (op.is_svg) {
              this.morphSvgChildren(el, op.html);
            } else {
              this.morphInnerHtml(el, op.html);
            }
          }
          break;
        }

        case 'remove': {
          const el = this.getById(op.target);
          if (el) {
            el.remove();
            this.idMap.delete(op.target);
          }
          break;
        }

        case 'insert': {
          const anchor = this.getById(op.anchor_id);
          if (!anchor) break;

          switch (op.anchor_type) {
            case 'after':
              anchor.insertAdjacentHTML('afterend', op.html);
              break;
            case 'before':
              anchor.insertAdjacentHTML('beforebegin', op.html);
              break;
            case 'first':
              anchor.insertAdjacentHTML('afterbegin', op.html);
              break;
            case 'last':
              anchor.insertAdjacentHTML('beforeend', op.html);
              break;
          }
          break;
        }

        case 'move': {
          const el = this.getById(op.target);
          const anchor = this.getById(op.anchor_id);
          if (!el || !anchor) break;

          switch (op.anchor_type) {
            case 'after':
              anchor.insertAdjacentElement('afterend', el);
              break;
            case 'before':
              anchor.insertAdjacentElement('beforebegin', el);
              break;
            case 'first':
              anchor.insertAdjacentElement('afterbegin', el);
              break;
            case 'last':
              anchor.insertAdjacentElement('beforeend', el);
              break;
          }
          break;
        }

        case 'attrs': {
          const el = this.getById(op.target);
          if (el) {
            for (const [name, value] of op.attrs) {
              if (value === null) {
                el.removeAttribute(name);
              } else {
                el.setAttribute(name, value);
              }
            }
          }
          break;
        }
      }
    },

    // Get element by StableId
    // Uses querySelectorAll to get the LAST matching element, consistent with hydrate()
    getById(id) {
      let el = this.idMap.get(id);
      if (el && el.isConnected) return el;

      // Get last matching element (same behavior as hydrate which iterates and overwrites)
      const all = document.querySelectorAll(`[data-tola-id="${id}"]`);
      if (all.length > 0) {
        el = all[all.length - 1];
      } else {
        el = null;
      }
      if (el) this.idMap.set(id, el);
      return el;
    },

    // Seamless CSS update: load the new stylesheet while the old one remains
    // applied, then commit the swap after the new CSS is ready.
    seamlessCssUpdate(oldLink, newHtml) {
      return this.prepareStylesheetUpdate(oldLink, newHtml).then((update) => {
        update.activate();
        update.cleanup();
      });
    },

    prepareStylesheetUpdate(oldLink, newHtml) {
      return new Promise((resolve) => {
      // Parse new link element from HTML
        const temp = document.createElement('div');
        temp.innerHTML = newHtml;
        const newLink = temp.querySelector('link');
        if (!newLink) {
          // Fallback to direct replacement if parsing fails
          resolve(this.styleUpdate(() => this.morphOuterHtml(oldLink, newHtml)));
          return;
        }

        const pending = newLink.cloneNode(false);
        const media = pending.getAttribute('media');
        pending.dataset.tolaPendingStylesheet = 'true';
        pending.dataset.tolaMedia = media === null ? '' : media;
        pending.media = 'not all';

        const finish = () => {
          resolve(this.styleUpdate(
            () => this.activateStylesheet(pending),
            () => this.cleanupStylesheet(oldLink, pending),
          ));
        };

        pending.onload = finish;
        pending.onerror = () => {
          pending.remove();
          resolve(this.styleUpdate(() => this.morphOuterHtml(oldLink, newHtml)));
        };

        oldLink.insertAdjacentElement('afterend', pending);
      });
    },

    styleUpdate(activate, cleanup) {
      return {
        activate: typeof activate === 'function' ? activate : () => {},
        cleanup: typeof cleanup === 'function' ? cleanup : () => {},
      };
    },

    emptyStyleUpdate() {
      return this.styleUpdate();
    },

    activateStylesheet(pendingLink) {
      if (!pendingLink || !pendingLink.isConnected) {
        return;
      }

      const media = pendingLink.dataset.tolaMedia;
      pendingLink.removeAttribute('data-tola-pending-stylesheet');
      pendingLink.removeAttribute('data-tola-media');
      if (media) {
        pendingLink.media = media;
      } else {
        pendingLink.removeAttribute('media');
      }
    },

    cleanupStylesheet(oldLink, pendingLink) {
      if (!pendingLink || !pendingLink.isConnected) {
        return;
      }

      const removeOld = () => {
        if (oldLink.isConnected) {
          oldLink.remove();
        }
      };

      if (typeof requestAnimationFrame === 'function') {
        requestAnimationFrame(() => {
          requestAnimationFrame(removeOld);
        });
      } else {
        setTimeout(removeOld, 32);
      }
    },

    morphOuterHtml(el, html) {
      const next = this.parseHtmlNode(html);
      if (!next) {
        el.outerHTML = html;
        return;
      }
      this.morphNode(el, next);
    },

    morphInnerHtml(el, html) {
      const range = document.createRange();
      range.selectNodeContents(el);
      this.morphChildren(el, range.createContextualFragment(html));
    },

    morphSvgChildren(el, html) {
      const temp = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      temp.innerHTML = html;
      this.morphChildren(el, temp);
    },

    parseHtmlNode(html) {
      const template = document.createElement('template');
      template.innerHTML = html.trim();
      return template.content.firstChild;
    },

    morphNode(oldNode, newNode) {
      if (!this.nodesMatch(oldNode, newNode)) {
        oldNode.replaceWith(newNode);
        return;
      }

      if (oldNode.nodeType === Node.TEXT_NODE) {
        if (oldNode.textContent !== newNode.textContent) {
          oldNode.textContent = newNode.textContent;
        }
        return;
      }

      if (oldNode.nodeType !== Node.ELEMENT_NODE) {
        return;
      }

      this.syncAttributes(oldNode, newNode);
      this.morphChildren(oldNode, newNode);
    },

    morphChildren(oldParent, newParent) {
      let cursor = oldParent.firstChild;
      for (const newChild of Array.from(newParent.childNodes)) {
        const oldChild = this.matchChild(oldParent, cursor, newChild);
        if (oldChild) {
          if (oldChild !== cursor) {
            oldParent.insertBefore(oldChild, cursor);
          }
          this.morphNode(oldChild, newChild);
          cursor = oldChild.nextSibling;
        } else {
          oldParent.insertBefore(newChild, cursor);
        }
      }

      while (cursor) {
        const next = cursor.nextSibling;
        cursor.remove();
        cursor = next;
      }
    },

    matchChild(parent, cursor, newChild) {
      if (cursor && this.nodesMatch(cursor, newChild)) {
        return cursor;
      }

      const id = this.nodeStableId(newChild);
      if (!id) {
        return null;
      }

      for (let node = cursor; node; node = node.nextSibling) {
        if (node.parentNode === parent && this.nodeStableId(node) === id) {
          return node;
        }
      }
      return null;
    },

    nodesMatch(oldNode, newNode) {
      if (!oldNode || !newNode || oldNode.nodeType !== newNode.nodeType) {
        return false;
      }

      if (oldNode.nodeType === Node.ELEMENT_NODE) {
        const oldId = this.nodeStableId(oldNode);
        const newId = this.nodeStableId(newNode);
        if (oldId || newId) {
          return oldId === newId;
        }
      }

      return oldNode.nodeName === newNode.nodeName;
    },

    nodeStableId(node) {
      return node && node.nodeType === Node.ELEMENT_NODE ? node.dataset.tolaId || '' : '';
    },

    syncAttributes(oldEl, newEl) {
      for (const attr of Array.from(oldEl.attributes)) {
        if (!newEl.hasAttribute(attr.name)) {
          oldEl.removeAttribute(attr.name);
        }
      }
      for (const attr of Array.from(newEl.attributes)) {
        if (oldEl.getAttribute(attr.name) !== attr.value) {
          oldEl.setAttribute(attr.name, attr.value);
        }
      }
    },

    // SyncTeX: get source location from element
    getSourceLocation(el) {
      while (el && el !== document.body) {
        var id = el.dataset && el.dataset.tolaId;
        if (id) return { id: id, tag: el.tagName.toLowerCase() };
        el = el.parentElement;
      }
      return null;
    },

    // Report current page URL to the server for active-page tracking.
    reportCurrentPage() {
      // Decode URL for server (server expects decoded URLs internally)
      const urlPath = decodeURIComponent(window.location.pathname);
      this.sendMessage({ type: 'page', path: urlPath });
    }
  };

  // Initialize
  Tola.setupHistoryReloadGuard();
  Tola.setupReconnectTriggers();
  Tola.connect(__TOLA_WS_PORT__);
  window.Tola = Tola;
})();

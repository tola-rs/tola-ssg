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
  const Tola = {
    // StableId -> Element mapping for O(1) lookups
    idMap: new Map(),
    ws: null,
    reconnectDelay: 1000,

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
      this.ws = new WebSocket(`ws://localhost:${port}/`);

      this.ws.onopen = () => {
        console.log('[tola] hot reload connected');
        this.reconnectDelay = 1000;
        this.hydrate();
        // Report current page to server for priority compilation
        this.reportCurrentPage();
      };

      this.ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          this.handleMessage(msg);
        } catch (err) {
          console.error('[tola] message error:', err);
        }
      };

      this.ws.onclose = () => {
        console.log('[tola] disconnected, attempting reconnect...');
        this.attemptReconnect();
      };

      this.ws.onerror = (err) => {
        console.error('[tola] WebSocket error:', err);
      };
    },

    // Attempt to reconnect with exponential backoff
    attemptReconnect() {
      const maxRetries = 30; // Try for up to ~2 minutes
      let retries = 0;

      const tryConnect = () => {
        retries++;
        console.log(`[tola] reconnect attempt ${retries}/${maxRetries}`);

        // Try to fetch a simple resource to check if server is ready
        // Must check X-Tola-Ready header to ensure build is complete
        fetch(window.location.href, { method: 'HEAD' })
          .then(r => {
            if (r.headers.get('X-Tola-Ready') === 'true') {
              // Server is ready, reload the page
              console.log('[tola] server is ready, reloading...');
              location.reload();
            } else {
              // Server is up but not ready yet (still building)
              console.log('[tola] server is building, waiting...');
              if (retries < maxRetries) {
                setTimeout(tryConnect, 1000);
              }
            }
          })
          .catch(() => {
            // Server not reachable yet
            if (retries < maxRetries) {
              const delay = Math.min(1000 * Math.pow(1.3, retries - 1), 5000);
              setTimeout(tryConnect, delay);
            } else {
              console.log('[tola] giving up reconnect, please refresh manually');
            }
          });
      };

      // Start trying after a short delay
      setTimeout(tryConnect, 500);
    },

    // Handle incoming message
    handleMessage(msg) {
      switch (msg.type) {
        case 'reload':
          console.log('[tola] reloading:', msg.reason || 'file changed');
          // If permalink changed, update URL before reload to avoid 404
          if (msg.url_change) {
            this.updateUrl(msg.url_change);
          }
          location.reload();
          break;
        case 'patch':
          // StableIds are globally unique (include page path hash), so we can
          // safely apply all patches - only matching elements will be affected.
          // This naturally supports htmx/dynamic content loading.
          this.hideErrorOverlay(); // Clear any previous error
          this.applyPatches(msg.ops);

          // Clear SPA prefetch cache (content may have changed)
          if (window.TolaSpa && typeof window.TolaSpa.clearCaches === 'function') {
            window.TolaSpa.clearCaches();
          }

          // Seamless URL update when permalink changes (no reload)
          if (msg.url_change) {
            this.updateUrl(msg.url_change);
          }
          break;
        case 'connected':
          console.log('[tola] server version:', msg.version);
          break;
        case 'error':
          console.error('[tola] compile error:', msg.path, msg.error);
          this.showErrorOverlay(msg.path, msg.error);
          break;
        case 'clear_error':
          console.log('[tola] error cleared');
          this.hideErrorOverlay();
          break;
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

    // Show error overlay without reloading
    showErrorOverlay(path, error) {
      let overlay = document.getElementById('tola-error-overlay');
      if (!overlay) {
        overlay = document.createElement('div');
        overlay.id = 'tola-error-overlay';
        overlay.innerHTML = `
          <style>
            #tola-error-overlay {
              position: fixed;
              bottom: 0;
              left: 0;
              right: 0;
              max-height: 50vh;
              display: flex;
              flex-direction: column;
              background: #18181b;
              color: #fafafa;
              font-family: ui-monospace, 'SF Mono', Menlo, Monaco, 'Cascadia Code', monospace;
              font-size: 13px;
              z-index: 99999;
              border-top: 2px solid #dc2626;
              box-shadow: 0 -8px 32px rgba(0,0,0,0.4);
            }
            #tola-error-overlay .tola-error-header {
              display: flex;
              justify-content: space-between;
              align-items: center;
              padding: 10px 16px;
              background: #27272a;
              flex-shrink: 0;
            }
            #tola-error-overlay .tola-error-title {
              color: #fafafa;
              font-weight: 600;
              font-size: 13px;
              letter-spacing: 0.02em;
            }
            #tola-error-overlay .tola-error-close {
              background: transparent;
              border: 1px solid #3f3f46;
              color: #a1a1aa;
              padding: 4px 10px;
              cursor: pointer;
              font-size: 12px;
              font-family: inherit;
              transition: all 0.15s;
            }
            #tola-error-overlay .tola-error-close:hover {
              background: #3f3f46;
              color: #fafafa;
              border-color: #52525b;
            }
            #tola-error-overlay .tola-error-content {
              overflow: auto;
              padding: 14px 16px;
              flex: 1;
              min-height: 0;
            }
            #tola-error-overlay .tola-error-path {
              color: #71717a;
              margin-bottom: 10px;
              font-size: 12px;
            }
            #tola-error-overlay .tola-error-message {
              white-space: pre-wrap;
              word-break: break-word;
              line-height: 1.6;
              color: #e4e4e7;
            }
          </style>
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
      overlay.querySelector('.tola-error-path').textContent = path;
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
    applyPatches(ops) {
      for (const op of ops) {
        try {
          this.applyPatch(op);
        } catch (err) {
          console.error('[tola] patch failed:', op.op, err);
          location.reload();
          return;
        }
      }
      this.hydrate();

      // Update recolor filter (CSS variables may have changed)
      if (window.TolaRecolor && typeof window.TolaRecolor.update === 'function') {
        window.TolaRecolor.update();
      }
    },

    // Apply single patch - pure ID/anchor based, no position indices
    applyPatch(op) {
      switch (op.op) {
        case 'replace': {
          const el = this.getById(op.target);
          if (el) el.outerHTML = op.html;
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
            // SVG and non-SVG: just use innerHTML directly
            el.innerHTML = op.html;
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

    // SyncTeX: get source location from element
    getSourceLocation(el) {
      while (el && el !== document.body) {
        var id = el.dataset && el.dataset.tolaId;
        if (id) return { id: id, tag: el.tagName.toLowerCase() };
        el = el.parentElement;
      }
      return null;
    },

    // Report current page URL to server for priority compilation
    reportCurrentPage() {
      if (this.ws && this.ws.readyState === WebSocket.OPEN) {
        // Decode URL for server (server expects decoded URLs internally)
        const urlPath = decodeURIComponent(window.location.pathname);
        this.ws.send(JSON.stringify({ type: 'page', path: urlPath }));
      }
    }
  };

  // Initialize
  Tola.connect(__TOLA_WS_PORT__);
  window.Tola = Tola;
})();


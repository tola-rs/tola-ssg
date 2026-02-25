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
    // Phase 1: apply stylesheet replacements and wait for preload completion
    // Phase 2: apply all remaining DOM patches
    applyPatches(ops) {
      const cssOps = [];
      const otherOps = [];

      for (const op of ops) {
        if (this.isStylesheetReplaceOp(op)) {
          cssOps.push(op);
        } else {
          otherOps.push(op);
        }
      }

      const applyRemaining = () => {
        for (const op of otherOps) {
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
      };

      if (cssOps.length === 0) {
        applyRemaining();
        return;
      }

      const cssTasks = [];
      for (const op of cssOps) {
        try {
          cssTasks.push(this.applyStylesheetReplace(op));
        } catch (err) {
          console.error('[tola] css patch failed:', op.op, err);
          location.reload();
          return;
        }
      }

      Promise.all(cssTasks)
        .then(applyRemaining)
        .catch((err) => {
          console.error('[tola] css patch failed:', err);
          location.reload();
        });
    },

    isStylesheetReplaceOp(op) {
      if (!op || op.op !== 'replace' || typeof op.html !== 'string') return false;
      const temp = document.createElement('div');
      temp.innerHTML = op.html;
      const link = temp.querySelector('link');
      return !!(link && link.rel === 'stylesheet');
    },

    applyStylesheetReplace(op) {
      const el = this.getById(op.target);
      if (!el) {
        return Promise.resolve();
      }
      if (el.tagName === 'LINK' && el.rel === 'stylesheet') {
        return this.seamlessCssUpdate(el, op.html);
      }
      // Fallback: if target exists but is not stylesheet, apply as normal replace
      this.applyPatch(op);
      return Promise.resolve();
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
              el.outerHTML = op.html;
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
              // SVG requires namespace-aware parsing
              const temp = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
              temp.innerHTML = op.html;
              el.replaceChildren(...temp.childNodes);
            } else {
              el.innerHTML = op.html;
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

    // Seamless CSS update: preload new stylesheet before removing old one
    // This prevents flash of unstyled content (FOUC)
    seamlessCssUpdate(oldLink, newHtml) {
      return new Promise((resolve) => {
      // Parse new link element from HTML
        const temp = document.createElement('div');
        temp.innerHTML = newHtml;
        const newLink = temp.querySelector('link');
        if (!newLink) {
          // Fallback to direct replacement if parsing fails
          oldLink.outerHTML = newHtml;
          resolve();
          return;
        }

        // Create a preload link to fetch CSS without applying it
        const preload = document.createElement('link');
        preload.rel = 'preload';
        preload.as = 'style';
        preload.href = newLink.href;

        const finish = () => {
          preload.remove();
          resolve();
        };

        // When preload completes, swap the stylesheets
        preload.onload = () => {
          // Remove attributes that no longer exist
          for (const attr of Array.from(oldLink.attributes)) {
            if (!newLink.hasAttribute(attr.name)) {
              oldLink.removeAttribute(attr.name);
            }
          }
          // Copy all attributes from new link
          for (const attr of newLink.attributes) {
            oldLink.setAttribute(attr.name, attr.value);
          }
          finish();
        };

        preload.onerror = () => {
          // Fallback to direct replacement on error
          oldLink.outerHTML = newHtml;
          finish();
        };

        // Start preloading
        document.head.appendChild(preload);
      });
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

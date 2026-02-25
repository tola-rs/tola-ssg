// ==========================================================================
// Tola SPA Navigation Runtime
// ==========================================================================
//
// Provides seamless page transitions without full page reloads.
// Uses idiomorph for intelligent DOM morphing.
//
// Configuration (injected at build time):
//   __TOLA_TRANSITION__: boolean - enable View Transitions API
//   __TOLA_PRELOAD__: boolean - enable hover prefetch
//   __TOLA_PRELOAD_DELAY__: number - prefetch delay in ms

(function() {
  'use strict';

  // ==========================================================================
  // Idiomorph (MIT License) - Inline minified version
  // https://github.com/bigskysoftware/idiomorph
  // ==========================================================================
  var Idiomorph=(function(){"use strict";function morphChildren(oldParent,newParent,ctx){let nextOld=oldParent.firstChild;let nextNew=newParent.firstChild;while(nextNew){let newChild=nextNew;nextNew=newChild.nextSibling;if(nextOld){let oldChild=nextOld;nextOld=oldChild.nextSibling;morphNode(oldChild,newChild,ctx)}else{oldParent.appendChild(newChild)}}while(nextOld){let oldChild=nextOld;nextOld=oldChild.nextSibling;oldParent.removeChild(oldChild)}}function morphNode(oldNode,newNode,ctx){if(oldNode.nodeType!==newNode.nodeType||oldNode.nodeName!==newNode.nodeName){oldNode.parentNode.replaceChild(newNode,oldNode);return}if(oldNode.nodeType===3){if(oldNode.textContent!==newNode.textContent){oldNode.textContent=newNode.textContent}return}if(oldNode.nodeType===1){morphAttributes(oldNode,newNode);morphChildren(oldNode,newNode,ctx)}}function morphAttributes(oldEl,newEl){for(let attr of Array.from(oldEl.attributes)){if(!newEl.hasAttribute(attr.name)){oldEl.removeAttribute(attr.name)}}for(let attr of Array.from(newEl.attributes)){if(oldEl.getAttribute(attr.name)!==attr.value){oldEl.setAttribute(attr.name,attr.value)}}}function morph(oldNode,newNode,config={}){let ctx={config};if(typeof newNode==="string"){let parser=new DOMParser();let doc=parser.parseFromString(newNode,"text/html");newNode=doc.body.firstChild}morphNode(oldNode,newNode,ctx);return oldNode}return{morph}})();

  // ==========================================================================
  // Configuration (replaced at build time)
  // ==========================================================================
  const CONFIG = {
    transition: __TOLA_TRANSITION__,
    preload: __TOLA_PRELOAD__,
    preloadDelay: __TOLA_PRELOAD_DELAY__,
    // Hardcoded defaults
    scrollCacheSize: 50,      // Max scroll positions to cache
    prefetchCacheSize: 20     // Max prefetch entries
  };

  // Build-time injected path prefix (e.g. "/blog"), empty string for root.
  const PATH_PREFIX = __TOLA_PATH_PREFIX__;

  // ==========================================================================
  // Caches
  // ==========================================================================
  const prefetchCache = new Map();
  // Scroll cache stores scroll positions only (not HTML) to avoid stale content
  const scrollCache = new Map();  // pathKey -> scrollY

  // localStorage key for scroll positions
  const SCROLL_STORAGE_KEY = 'tola-scroll';

  // Load scroll positions from localStorage on init
  (function loadScrollCache() {
    try {
      var stored = localStorage.getItem(SCROLL_STORAGE_KEY);
      if (stored) {
        var data = JSON.parse(stored);
        if (data && typeof data === 'object') {
          Object.keys(data).forEach(function(key) {
            scrollCache.set(key, data[key]);
          });
        }
      }
    } catch (e) {
      // Ignore localStorage errors (private browsing, etc.)
    }
  })();

  // Save scroll cache to localStorage
  function saveScrollCache() {
    try {
      var data = {};
      scrollCache.forEach(function(value, key) {
        data[key] = value;
      });
      localStorage.setItem(SCROLL_STORAGE_KEY, JSON.stringify(data));
    } catch (e) {
      // Ignore localStorage errors
    }
  }

  // Current request (for abort)
  let currentController = null;
  // Monotonic navigation sequence for stale-response protection
  let currentNavigationId = 0;

  // ==========================================================================
  // Utility: normalize URL to path key (always absolute path)
  // ==========================================================================
  function toPathKey(url) {
    try {
      const u = new URL(url, location.origin);
      return u.pathname + u.search;
    } catch (e) {
      return url;
    }
  }

  function withPathPrefix(path) {
    if (!PATH_PREFIX) return path;
    if (path === '/') return PATH_PREFIX + '/';
    return PATH_PREFIX + (path.charAt(0) === '/' ? path : '/' + path);
  }

  function nextNavigationId() {
    currentNavigationId += 1;
    return currentNavigationId;
  }

  function isStaleNavigation(navId) {
    return navId !== currentNavigationId;
  }

  function closestAnchor(target) {
    return target && typeof target.closest === 'function' ? target.closest('a') : null;
  }

  // ==========================================================================
  // Link Interception
  // ==========================================================================
  function shouldIntercept(link) {
    if (!link || link.tagName !== 'A') return false;
    if (link.target === '_blank') return false;
    if (link.origin !== location.origin) return false;
    if (link.hasAttribute('download')) return false;
    if (link.hasAttribute('data-spa-ignore')) return false;
    // Same page anchor - let browser handle
    if (link.hash && link.pathname === location.pathname) return false;
    return true;
  }

  document.addEventListener('click', function(e) {
    // Ignore modified clicks (new tab, etc.)
    if (e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return;
    // Ignore non-primary button clicks
    if (e.button !== 0) return;

    const link = closestAnchor(e.target);
    if (!shouldIntercept(link)) return;

    e.preventDefault();
    navigateTo(link.href);
  });

  // ==========================================================================
  // Navigation
  // ==========================================================================
  function navigateTo(url, options) {
    options = options || {};
    const pushState = options.pushState !== false;
    const restoreScroll = options.restoreScroll === true;
    const pathKey = toPathKey(url);

    // Skip if navigating to the same page (prevents duplicate history entries)
    if (pushState && toPathKey(location.href) === pathKey) {
      return;
    }
    const navId = nextNavigationId();

    // Abort previous request if still pending
    if (currentController) {
      currentController.abort();
    }
    currentController = new AbortController();

    // Save current scroll position before navigating
    if (pushState) {
      scrollCache.set(toPathKey(location.href), window.scrollY);
      trimScrollCache();
      saveScrollCache();
    }

    // Check prefetch cache first (uses pathKey to ignore fragment)
    var html = prefetchCache.get(pathKey);
    if (html) {
      prefetchCache.delete(pathKey);
      finishNavigation(html, url, pushState, restoreScroll, pathKey, navId);
      return;
    }

    // Fetch the page
    fetch(url, {
      signal: currentController.signal,
      headers: { 'X-Tola-SPA': 'true' }
    })
    .then(function(response) {
      if (response.status === 404) {
        // Fetch 404 page for seamless transition
        return fetch(withPathPrefix('/404.html'), { signal: currentController.signal })
          .then(function(r) {
            if (r.ok) return r.text();
            // 404 page not found, fallback
            location.href = url;
            return null;
          });
      }
      if (!response.ok) {
        // Other errors - fallback to normal navigation
        location.href = url;
        return;
      }
      return response.text();
    })
    .then(function(html) {
      if (html && !isStaleNavigation(navId)) {
        finishNavigation(html, url, pushState, restoreScroll, pathKey, navId);
      }
    })
    .catch(function(err) {
      if (err.name === 'AbortError' || isStaleNavigation(navId)) {
        // Request was aborted, ignore
        return;
      }
      console.error('[tola-spa] navigation error:', err);
      // Fallback to normal navigation
      location.href = url;
    });
  }

  function finishNavigation(html, url, pushState, restoreScroll, pathKey, navId) {
    if (isStaleNavigation(navId)) return;
    currentController = null;

    var newDoc = new DOMParser().parseFromString(html, 'text/html');

    // First, preload any new stylesheets before morphing
    var stylePromises = preloadNewStylesheets(newDoc.head);

    // Wait for styles to load, then morph
    Promise.all(stylePromises).then(function() {
      if (isStaleNavigation(navId)) return;

      // Morph the page (with or without View Transitions)
      if (CONFIG.transition && document.startViewTransition) {
        document.startViewTransition(function() {
          morphPage(newDoc);
        });
      } else {
        morphPage(newDoc);
      }

      // Update URL after morphing
      if (pushState) {
        history.pushState({ tola: true }, '', url);
      }

      // Dispatch custom event for user scripts (after URL update)
      document.dispatchEvent(new CustomEvent('tola:navigate', {
        detail: { url: location.href }
      }));

      // Handle scroll position
      var scrollPos = null;
      if (restoreScroll) {
        scrollPos = scrollCache.get(pathKey);
      }
      handleScroll(url, scrollPos);

      // Report new page to hotreload server (if connected)
      if (window.Tola && typeof window.Tola.reportCurrentPage === 'function') {
        window.Tola.reportCurrentPage();
      }
    });
  }

  // Extract pathname from href (strip query string for version comparison)
  function getStylesheetPathname(href) {
    try {
      var url = new URL(href, location.origin);
      return url.pathname;
    } catch (e) {
      // Fallback: strip query string manually
      var idx = href.indexOf('?');
      return idx >= 0 ? href.substring(0, idx) : href;
    }
  }

  // Sync all attributes from source to target (including data-tola-id)
  function syncElementAttributes(target, source) {
    var i;

    // Remove attributes that no longer exist
    var targetAttrs = Array.prototype.slice.call(target.attributes);
    for (i = 0; i < targetAttrs.length; i++) {
      var name = targetAttrs[i].name;
      if (!source.hasAttribute(name)) {
        target.removeAttribute(name);
      }
    }

    // Set current attributes
    var sourceAttrs = Array.prototype.slice.call(source.attributes);
    for (i = 0; i < sourceAttrs.length; i++) {
      target.setAttribute(sourceAttrs[i].name, sourceAttrs[i].value);
    }
  }

  // Preload stylesheets from new document before morphing
  function preloadNewStylesheets(newHead) {
    var promises = [];
    var newLinks = newHead.querySelectorAll('link[rel="stylesheet"]');

    newLinks.forEach(function(newLink) {
      var href = newLink.getAttribute('href');
      if (!href) return;

      var newPathname = getStylesheetPathname(href);

      // Check if this stylesheet (by pathname, ignoring version) already exists
      var existingLinks = document.querySelectorAll('link[rel="stylesheet"]');
      var existing = null;
      var exactMatch = null;
      for (var i = 0; i < existingLinks.length; i++) {
        var existingHref = existingLinks[i].getAttribute('href');
        if (!existingHref) continue;
        if (existingHref === href) {
          exactMatch = existingLinks[i];
          break;
        }
        if (!existing && getStylesheetPathname(existingHref) === newPathname) {
          existing = existingLinks[i];
        }
      }

      // Same stylesheet URL: keep node, but sync attrs (important for data-tola-id)
      if (exactMatch) {
        syncElementAttributes(exactMatch, newLink);
        return;
      }

      // Create and append the new stylesheet, wait for it to load
      var link = newLink.cloneNode(true);

      var promise = new Promise(function(resolve) {
        link.onload = function() {
          // Remove old version after new one loads (seamless update)
          if (existing && existing.parentNode) {
            existing.parentNode.removeChild(existing);
          }
          resolve();
        };
        link.onerror = resolve;
        setTimeout(function() {
          // Timeout fallback: still remove old version
          if (existing && existing.parentNode) {
            existing.parentNode.removeChild(existing);
          }
          resolve();
        }, 500);
      });

      document.head.appendChild(link);
      promises.push(promise);
    });

    return promises;
  }

  // ==========================================================================
  // Scroll Handling
  // ==========================================================================
  function trimScrollCache() {
    while (scrollCache.size > CONFIG.scrollCacheSize) {
      var firstKey = scrollCache.keys().next().value;
      scrollCache.delete(firstKey);
    }
  }

  function handleScroll(url, savedScroll) {
    var hash;
    try {
      hash = new URL(url, location.origin).hash;
    } catch (e) {
      hash = '';
    }

    if (hash) {
      // Scroll to anchor
      var target = document.querySelector(hash);
      if (target) {
        target.scrollIntoView();
        return;
      }
    }

    if (typeof savedScroll === 'number') {
      // Restore saved scroll position (back/forward)
      window.scrollTo(0, savedScroll);
    } else {
      // New navigation - scroll to top
      window.scrollTo(0, 0);
    }
  }

  // ==========================================================================
  // DOM Morphing
  // ==========================================================================
  function morphPage(newDoc) {
    // 1. Update title
    document.title = newDoc.title;

    // 2. Merge head
    mergeHead(newDoc.head);

    // 3. Morph body
    Idiomorph.morph(document.body, newDoc.body);

    // 4. Execute inline scripts in body (they don't run after DOM morph)
    executeInlineScripts(document.body);

    // 5. Re-hydrate tola IDs (for hot reload compatibility)
    if (window.Tola && typeof window.Tola.hydrate === 'function') {
      window.Tola.hydrate();
    }

    // Note: tola:navigate event is dispatched in finishNavigation after URL update
  }

  // Execute inline scripts after morph (external scripts are skipped)
  function executeInlineScripts(container) {
    var scripts = container.querySelectorAll('script:not([src])');
    scripts.forEach(function(oldScript) {
      var newScript = document.createElement('script');
      // Copy attributes
      Array.from(oldScript.attributes).forEach(function(attr) {
        newScript.setAttribute(attr.name, attr.value);
      });
      // Copy content
      newScript.textContent = oldScript.textContent;
      // Replace to execute
      oldScript.parentNode.replaceChild(newScript, oldScript);
    });
  }

  // ==========================================================================
  // Head Merging
  // ==========================================================================
  function mergeHead(newHead) {
    var oldHead = document.head;

    // Collect existing elements by key (keep arrays to support duplicate keys)
    var oldElements = new Map();
    var i, el, key, arr;
    for (i = 0; i < oldHead.children.length; i++) {
      el = oldHead.children[i];
      key = getHeadElementKey(el);
      if (!key) continue;
      arr = oldElements.get(key);
      if (!arr) {
        arr = [];
        oldElements.set(key, arr);
      }
      arr.push(el);
    }

    // Track elements to remove later
    var toRemove = [];

    // Process new head elements
    var newChildren = Array.prototype.slice.call(newHead.children);
    for (i = 0; i < newChildren.length; i++) {
      var newEl = newChildren[i];
      key = getHeadElementKey(newEl);
      if (!key) continue;

      var oldList = oldElements.get(key);
      var oldEl = oldList && oldList.length > 0 ? oldList.shift() : null;
      if (oldList && oldList.length === 0) {
        oldElements.delete(key);
      }
      if (oldEl) {
        // Update existing element if content changed
        // For stylesheets: preloadNewStylesheets already handled the update, just skip
        // But we still need to mark it as processed
        if (oldEl.outerHTML !== newEl.outerHTML) {
          if (!(newEl.tagName === 'LINK' && newEl.rel === 'stylesheet')) {
            oldEl.parentNode.replaceChild(newEl.cloneNode(true), oldEl);
          }
          // Note: stylesheets are updated by preloadNewStylesheets which removes old version
        }
      } else {
        // Add new element (skip scripts and stylesheets - stylesheets already preloaded)
        if (newEl.tagName !== 'SCRIPT' && !(newEl.tagName === 'LINK' && newEl.rel === 'stylesheet')) {
          oldHead.appendChild(newEl.cloneNode(true));
        }
      }
    }

    // Collect elements to remove (except scripts, global styles)
    // Note: stylesheets are handled by preloadNewStylesheets which removes old versions
    oldElements.forEach(function(list) {
      for (var i = 0; i < list.length; i++) {
        var el = list[i];
        // Keep: scripts, inline styles, and .tola assets
        if (el.tagName === 'SCRIPT' || el.tagName === 'STYLE') continue;
        if (el.tagName === 'LINK' && el.href && el.href.indexOf('/.tola/') !== -1) continue;
        // Mark for removal (including orphaned stylesheets not in new head)
        toRemove.push(el);
      }
    });

    // Remove old elements
    toRemove.forEach(function(el) {
      if (el.parentNode) el.parentNode.removeChild(el);
    });
  }

  function getHeadElementKey(el) {
    var name;
    switch (el.tagName) {
      case 'TITLE':
        return 'title';
      case 'META':
        name = el.name || el.httpEquiv || el.getAttribute('property') || '';
        if (name) {
          return 'meta:' + name;
        }
        return null;
      case 'LINK':
        // For stylesheets, use pathname (without version query) as key
        // This allows proper matching when CSS version changes
        if (el.rel === 'stylesheet') {
          return 'link:stylesheet:' + getStylesheetPathname(el.href);
        }
        return 'link:' + el.rel + ':' + el.href;
      case 'SCRIPT':
        if (el.src) {
          return 'script:' + el.src;
        }
        return null;
      case 'STYLE':
        return null; // Don't track inline styles
      default:
        return null;
    }
  }

  // ==========================================================================
  // Browser History
  // ==========================================================================
  window.addEventListener('popstate', function(e) {
    navigateTo(location.href, { pushState: false, restoreScroll: true });
  });

  // ==========================================================================
  // Preload (Hover Prefetch)
  // ==========================================================================
  // Use a Map with pathKey as key (ignores fragment)
  var prefetchTimersByPath = new Map();

  if (CONFIG.preload) {
    document.addEventListener('mouseover', function(e) {
      var link = closestAnchor(e.target);
      if (!link) return;
      if (!shouldIntercept(link)) return;

      var pathKey = toPathKey(link.href);

      // Skip if it's the current page
      if (pathKey === toPathKey(location.href)) return;

      // Skip if already cached or being fetched
      if (prefetchCache.has(pathKey)) return;
      if (prefetchTimersByPath.has(pathKey)) return;

      // Delay prefetch to avoid false triggers
      var timer = setTimeout(function() {
        prefetchTimersByPath.delete(pathKey);

        // Limit prefetch cache size
        if (prefetchCache.size >= CONFIG.prefetchCacheSize) {
          var firstKey = prefetchCache.keys().next().value;
          prefetchCache.delete(firstKey);
        }

        fetch(link.href, { headers: { 'X-Tola-Prefetch': 'true' } })
          .then(function(r) {
            if (r.ok) {
              return r.text();
            }
            return null;
          })
          .then(function(html) {
            if (html) {
              prefetchCache.set(pathKey, html);
              // Preload stylesheets from the fetched HTML
              preloadStylesheets(html);
            }
          })
          .catch(function() {}); // Ignore prefetch errors
      }, CONFIG.preloadDelay);

      prefetchTimersByPath.set(pathKey, timer);
    });

    document.addEventListener('mouseout', function(e) {
      var link = closestAnchor(e.target);
      if (!link) return;

      var pathKey = toPathKey(link.href);
      var timer = prefetchTimersByPath.get(pathKey);
      if (timer) {
        clearTimeout(timer);
        prefetchTimersByPath.delete(pathKey);
      }
    });
  }

  // Preload stylesheets from HTML to avoid FOUC
  function preloadStylesheets(html) {
    var parser = new DOMParser();
    var doc = parser.parseFromString(html, 'text/html');
    var links = doc.querySelectorAll('link[rel="stylesheet"]');

    links.forEach(function(link) {
      var href = link.href;
      var pathname = getStylesheetPathname(href);

      // Skip if same version already in current document
      if (document.querySelector('link[href="' + href + '"]')) return;

      // Skip if same pathname (different version) already being preloaded
      var preloadLinks = document.querySelectorAll('link[rel="preload"][as="style"]');
      for (var i = 0; i < preloadLinks.length; i++) {
        if (getStylesheetPathname(preloadLinks[i].href) === pathname) return;
      }

      // Create a preload link
      var preload = document.createElement('link');
      preload.rel = 'preload';
      preload.as = 'style';
      preload.href = href;
      document.head.appendChild(preload);
    });
  }

  // ==========================================================================
  // Clear caches (called by hotreload on content change)
  // ==========================================================================
  function clearCaches() {
    prefetchCache.clear();
    // Keep scroll positions - they're still valid
  }

  // ==========================================================================
  // Migrate scroll position when URL changes (called by hotreload)
  // ==========================================================================
  function migrateScrollPosition(oldPath, newPath) {
    var oldKey = toPathKey(oldPath);
    var newKey = toPathKey(newPath);
    if (scrollCache.has(oldKey)) {
      scrollCache.set(newKey, scrollCache.get(oldKey));
      scrollCache.delete(oldKey);
      saveScrollCache();
    }
  }

  // ==========================================================================
  // Expose for debugging and hotreload integration
  // ==========================================================================
  window.TolaSpa = {
    navigateTo: navigateTo,
    prefetchCache: prefetchCache,
    scrollCache: scrollCache,
    config: CONFIG,
    clearCaches: clearCaches,
    migrateScrollPosition: migrateScrollPosition,
    abort: function() { if (currentController) currentController.abort(); }
  };

})();

// Development runtime for pages served by `tola dev`.

(function() {
  const owner = Symbol.for('tola.dev.runtime');
  if (document[owner]) return;
  const runtimeScript = document.currentScript;
  if (!(runtimeScript instanceof HTMLScriptElement)
      || !runtimeScript.hasAttribute('data-tola-runtime')) return;
  document[owner] = true;
  let bootstrap;
  try {
    bootstrap = JSON.parse(runtimeScript.getAttribute('data-tola-bootstrap'));
  } catch (_) {
    bootstrap = null;
  }
  if (!bootstrap || typeof bootstrap !== 'object'
      || !Number.isInteger(bootstrap.port) || bootstrap.port < 0 || bootstrap.port > 65535
      || typeof bootstrap.session !== 'string' || !bootstrap.session
      || (bootstrap.revision !== null && !isHexIdentity(bootstrap.revision))
      || (bootstrap.output !== null && (typeof bootstrap.output !== 'string' || !bootstrap.output))
      || ![null, 'empty', 'present'].includes(bootstrap.page_availability)
      || typeof bootstrap.mount !== 'string'
      || typeof bootstrap.generation !== 'string'
      || !/^[A-Za-z0-9_-]{1,128}$/.test(bootstrap.generation)) {
    console.error('[tola] development updates could not start; reload this page or restart `tola dev`');
    return;
  }
  Object.freeze(bootstrap);

  const DEV_STATUS_CSS = `__TOLA_DEV_STATUS_CSS__`;

  const RUNTIME_DOM_SELECTOR = '[data-tola-runtime], [data-tola-staging], style[data-tola-dev-status], #tola-dev-status';
  /// Narrower than RUNTIME_DOM_SELECTOR: only markup that can hold content hides.
  const RUNTIME_CONTAINER_SELECTOR = '[data-tola-runtime], #tola-dev-status';

  function createElement(tag, className, text) {
    const element = document.createElement(tag);
    element.className = className;
    if (text !== undefined) element.textContent = text;
    return element;
  }

  /// A count the development server reported for one severity.
  function isDiagnosticCount(value) {
    return Number.isInteger(value) && value >= 0;
  }

  function injectRuntimeStyle(attribute, css) {
    if (document.querySelector(`style[${attribute}]`)) return;
    const style = document.createElement('style');
    style.setAttribute(attribute, '');
    style.textContent = css;
    document.head.appendChild(style);
  }

  function withoutRepresentation(value) {
    try {
      const url = new URL(value, document.baseURI);
      url.searchParams.delete('tola-representation');
      return url.href;
    } catch (_) {
      return value;
    }
  }

  /// The server injects the mount in its encoded spelling, while every address this runtime
  /// compares is decoded, so the mount is decoded once here. `null` marks a mount no address can
  /// be compared against, and no address may then be judged.
  function decodedMount(injected) {
    if (typeof injected !== 'string') return '';
    const trimmed = injected.replace(/^\/+|\/+$/g, '');
    if (!trimmed) return '';
    try { return decodeURIComponent(trimmed); }
    catch (_) { return null; }
  }

  function elementIdentity(element) {
    if (!(element instanceof Element)) return null;
    const tag = element.tagName.toLowerCase();
    if (element.id && element.id.length <= 1024 && document.getElementById(element.id) === element) {
      return { by: 'id', value: element.id, tag };
    }
    const name = element.getAttribute('name');
    if (!name || name.length > 1024) return null;
    const matches = Array.from(document.getElementsByName(name))
      .filter(candidate => candidate.tagName.toLowerCase() === tag);
    return matches.length === 1 && matches[0] === element
      ? { by: 'name', value: name, tag }
      : null;
  }

  function resolveIdentity(identity) {
    if (!identity || typeof identity !== 'object'
        || !['id', 'name'].includes(identity.by)
        || typeof identity.value !== 'string' || !identity.value || identity.value.length > 1024
        || typeof identity.tag !== 'string' || !identity.tag) return null;
    if (identity.by === 'id') {
      const element = document.getElementById(identity.value);
      return element?.tagName.toLowerCase() === identity.tag ? element : null;
    }
    const matches = Array.from(document.getElementsByName(identity.value))
      .filter(candidate => candidate.tagName.toLowerCase() === identity.tag);
    return matches.length === 1 ? matches[0] : null;
  }

  function activeAttribute(element, name, value) {
    const attribute = name.toLowerCase();
    return (attribute.startsWith('on') && attribute in element)
      || attribute === 'is'
      || (['href', 'xlink:href', 'src', 'action', 'formaction'].includes(attribute)
        && /^\s*(javascript|vbscript):/i.test((value || '').replace(/[\t\n\r]/g, '')));
  }

  /// Active behavior prevents document/resource patches and native state restoration.
  function activeContent(root) {
    const candidates = root.nodeType === 1 ? [root, ...root.querySelectorAll('*')] : Array.from(root.querySelectorAll('*'));
    for (const element of candidates) {
      if (element.closest(RUNTIME_CONTAINER_SELECTOR)) continue;
      const tag = element.localName.toLowerCase();
      if (tag.includes('-') || element.hasAttribute('is') || element.shadowRoot) return element;
      if (['iframe', 'object', 'embed', 'applet'].includes(tag)) return element;
      if (Array.from(element.attributes).some(attribute => activeAttribute(element, attribute.name, attribute.value))) {
        return element;
      }
    }
    return null;
  }

  function normalizedMarkup(source) {
    const copy = source.documentElement.cloneNode(true);
    copy.querySelectorAll(RUNTIME_DOM_SELECTOR).forEach(element => element.remove());
    for (const element of copy.querySelectorAll('[href],[src],[poster],[data]')) {
      for (const attribute of ['href', 'src', 'poster', 'data']) {
        const value = element.getAttribute(attribute);
        if (value) element.setAttribute(attribute, withoutRepresentation(value));
      }
    }
    return copy.outerHTML;
  }

  function isHexIdentity(value) {
    return typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
  }

  function activeScript(script, type = script.getAttribute('type')) {
    if (script.matches(RUNTIME_DOM_SELECTOR)) return false;
    if (script.namespaceURI !== 'http://www.w3.org/1999/xhtml' || script.hasAttribute('src')) return true;
    const normalized = (type || '').trim().toLowerCase();
    return !['application/json', 'application/ld+json'].includes(normalized);
  }

  function siteBehavior(root) {
    if (activeContent(root)) return true;
    const scripts = root.nodeType === 1 && root.localName === 'script'
      ? [root, ...root.querySelectorAll('script')]
      : Array.from(root.querySelectorAll('script'));
    return scripts.some(script => !script.closest(RUNTIME_CONTAINER_SELECTOR) && activeScript(script));
  }

  function createSiteAddresses(readPathPrefix) {
    const isPageAvailability = value => value === 'empty' || value === 'present';

    function isValidOutputPath(path) {
      if (typeof path !== 'string' || !path || path.startsWith('/') || path.endsWith('/')) return false;
      // A published name may carry a literal `%` or `#`, and this decoded spelling is never turned
      // back into a URL: resource addresses keep the encoded spelling the server rendered.
      if (/[\\?\u0000-\u001f\u007f-\u009f]/u.test(path)) return false;
      return path.split('/').every(segment => isValidOutputSegment(segment));
    }

    function isValidOutputSegment(segment) {
      if (!segment || segment === '.' || segment === '..' || /[<>:"|?*]/u.test(segment)) return false;
      if (segment.endsWith('.') || segment.endsWith(' ')) return false;
      if (new TextEncoder().encode(segment).length > 255) return false;
      const stem = segment.split('.')[0].replace(/[ .]+$/u, '').toLowerCase();
      if (['con', 'prn', 'aux', 'nul', 'conin$', 'conout$'].includes(stem)) return false;
      return !/^(com|lpt)([1-9¹²³])$/u.test(stem);
    }

    function isValidOutput(output) {
      return output
        && typeof output === 'object'
        && isValidOutputPath(output.path)
        && isHexIdentity(output.representation)
        && Number.isSafeInteger(output.size)
        && output.size >= 0
        && ['html-document', 'pdf-document', 'png-document', 'svg-document', 'asset'].includes(output.kind);
    }

    function validatedOutput(change) {
      if (!change || typeof change !== 'object') return null;
      if (change.operation === 'added' || change.operation === 'removed') {
        return isValidOutput(change.output) ? change.output : null;
      }
      if (change.operation !== 'modified' || !change.output || typeof change.output !== 'object') {
        return null;
      }
      const before = change.output.before;
      const after = change.output.after;
      if (!isValidOutput(before) || !isValidOutput(after)) return null;
      if (before.path !== after.path || before.kind !== after.kind) return null;
      return after;
    }

    function outputPath(url) {
      try {
        const resolved = new URL(url, document.baseURI);
        if (resolved.origin !== location.origin) return null;
        const prefix = readPathPrefix();
        // An undecoded mount leaves no address comparable.
        if (prefix === null) return null;
        const decoded = decodeURIComponent(resolved.pathname);
        if (!decoded.startsWith('/') || decoded.includes('//')) return null;
        let path = decoded.slice(1);
        if (prefix) {
          // The mount's own directory URL carries its slash; `/<mount>` without one is a
          // host-root address outside the mount.
          if (path === `${prefix}/`) path = '';
          else {
            if (!path.startsWith(`${prefix}/`)) return null;
            path = path.slice(prefix.length + 1);
          }
        }
        // One route names one output: a trailing `/` is the directory index, anything else the
        // file it spells. The server resolves requests by the same rule.
        if (path === '' || path.endsWith('/')) {
          const directory = path.replace(/\/$/, '');
          const index = directory ? `${directory}/index.html` : 'index.html';
          return isValidOutputPath(index) ? index : null;
        }
        return isValidOutputPath(path) ? path : null;
      } catch (_) {
        return null;
      }
    }

      function withRepresentation(url, representation) {
      const next = new URL(url, document.baseURI);
      next.searchParams.set('tola-representation', representation);
      return next.href;
    }

    function representationOf(representations, url) {
      const path = outputPath(url);
      if (path === null) return null;
      return representations.get(path) ?? null;
    }

    return {
      isHexIdentity,
      isPageAvailability,
      validatedOutput,
      outputPath,
      withRepresentation,
      representationOf,
      get mountDecoded() { return readPathPrefix() !== null; },
    };
  }

  function createPageState({ mark, canRestore }) {
    // Capture and restore share one protocol bound: a page must never restore
    // state the runtime refuses to capture, and secret fields never round-trip.
    const FORM_EXCLUDED_INPUT_TYPES = new Set(['password', 'file', 'hidden', 'button', 'submit', 'reset', 'image']);
    const MAX_FORM_CONTROLS = 128;
    const MAX_CONTROL_VALUE_LENGTH = 65536;
    const MAX_SELECT_OPTIONS = 512;
    const MAX_SELECT_OPTION_LENGTH = 4096;

    function formRelation(control) {
      if (!('form' in control) || !control.form) return null;
      return elementIdentity(control.form) || undefined;
    }

    function matchesFormRelation(control, relation) {
      if (relation === null) return !('form' in control) || !control.form;
      if (!relation || typeof relation !== 'object') return false;
      return resolveIdentity(relation) === control.form;
    }

    function captureFormControls() {
      const controls = [];
      for (const control of Array.from(document.querySelectorAll('input,textarea,select'))) {
        if (controls.length >= MAX_FORM_CONTROLS) break;
        const identity = elementIdentity(control);
        const form = formRelation(control);
        if (!identity || form === undefined) continue;
        if (control instanceof HTMLInputElement) {
          const type = control.type.toLowerCase();
          if (FORM_EXCLUDED_INPUT_TYPES.has(type)) continue;
          if (type === 'checkbox' || type === 'radio') {
            if (control.checked !== control.defaultChecked) {
              controls.push({ identity, form, tag: 'input', type, mode: 'checked', checked: control.checked });
            }
          } else if (control.value !== control.defaultValue && control.value.length <= MAX_CONTROL_VALUE_LENGTH) {
            controls.push({ identity, form, tag: 'input', type, mode: 'value', value: control.value });
          }
          continue;
        }
        if (control instanceof HTMLTextAreaElement) {
          if (control.value !== control.defaultValue && control.value.length <= MAX_CONTROL_VALUE_LENGTH) {
            controls.push({ identity, form, tag: 'textarea', mode: 'value', value: control.value });
          }
          continue;
        }
        if (control instanceof HTMLSelectElement) {
          const selected = Array.from(control.options).flatMap((option, index) => option.selected ? [index] : []);
          const defaults = Array.from(control.options).flatMap((option, index) => option.defaultSelected ? [index] : []);
          if (JSON.stringify(selected) !== JSON.stringify(defaults)) {
            const options = Array.from(control.options).map(option => option.value);
            if (options.length <= MAX_SELECT_OPTIONS
                && options.every(value => value.length <= MAX_SELECT_OPTION_LENGTH)) {
              controls.push({ identity, form, tag: 'select', multiple: control.multiple, options, selected });
            }
          }
        }
      }
      return controls;
    }

    function captureFocus() {
      const element = document.activeElement;
      if (!(element instanceof HTMLElement)) return null;
      if (element instanceof HTMLInputElement && ['password', 'file'].includes(element.type.toLowerCase())) return null;
      const identity = elementIdentity(element);
      const form = formRelation(element);
      if (!identity || form === undefined) return null;
      const focus = { identity, form };
      if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {
        try {
          if (Number.isInteger(element.selectionStart) && Number.isInteger(element.selectionEnd)) {
            focus.selection = {
              start: element.selectionStart,
              end: element.selectionEnd,
              direction: element.selectionDirection,
            };
          }
        } catch (_) {}
      }
      return focus;
    }

    function mediaSource(media) {
      const source = media.currentSrc || media.getAttribute('src');
      if (!source) return null;
      try { return new URL(source, document.baseURI).href; }
      catch (_) { return null; }
    }

    function captureMedia() {
      const media = [];
      for (const element of Array.from(document.querySelectorAll('video,audio'))) {
        if (media.length >= 32) break;
        const identity = elementIdentity(element);
        const source = mediaSource(element);
        if (!identity || !source || !Number.isFinite(element.currentTime)) continue;
        media.push({
          identity,
          source,
          currentTime: element.currentTime,
          paused: element.paused,
          muted: element.muted,
          volume: element.volume,
          playbackRate: element.playbackRate,
        });
      }
      return media;
    }

    function capture() {
      return {
        controls: captureFormControls(),
        focus: captureFocus(),
        media: captureMedia(),
      };
    }

    function capturePatch() {
      const navigation = capture();
      const capturedNodes = new Set([...navigation.controls, ...navigation.media, navigation.focus]
        .map(saved => resolveIdentity(saved?.identity)).filter(Boolean));
      return { navigation, capturedNodes };
    }

    function restoreFormControls(controls, capturedNodes) {
      if (!Array.isArray(controls)) return;
      for (const saved of controls.slice(0, MAX_FORM_CONTROLS)) {
        if (!saved || typeof saved !== 'object') continue;
        const control = resolveIdentity(saved.identity);
        if (!control || !matchesFormRelation(control, saved.form)) continue;
        if (capturedNodes?.has(control)) continue;
        if (saved.tag === 'input' && control instanceof HTMLInputElement
            && control.type.toLowerCase() === saved.type) {
          if (saved.mode === 'checked' && typeof saved.checked === 'boolean'
              && ['checkbox', 'radio'].includes(saved.type)) control.checked = saved.checked;
          if (saved.mode === 'value' && typeof saved.value === 'string' && saved.value.length <= MAX_CONTROL_VALUE_LENGTH
              && !FORM_EXCLUDED_INPUT_TYPES.has(saved.type)) {
            control.value = saved.value;
          }
          continue;
        }
        if (saved.tag === 'textarea' && control instanceof HTMLTextAreaElement
            && saved.mode === 'value' && typeof saved.value === 'string' && saved.value.length <= MAX_CONTROL_VALUE_LENGTH) {
          control.value = saved.value;
          continue;
        }
        if (saved.tag === 'select' && control instanceof HTMLSelectElement
            && typeof saved.multiple === 'boolean' && control.multiple === saved.multiple
            && Array.isArray(saved.options) && saved.options.length <= MAX_SELECT_OPTIONS
            && saved.options.every(value => typeof value === 'string' && value.length <= MAX_SELECT_OPTION_LENGTH)
            && Array.isArray(saved.selected) && saved.selected.length <= MAX_SELECT_OPTIONS
            && saved.selected.every(index => Number.isInteger(index) && index >= 0 && index < saved.options.length)) {
          const options = Array.from(control.options).map(option => option.value);
          if (JSON.stringify(options) !== JSON.stringify(saved.options)) continue;
          const selected = new Set(saved.selected);
          for (const [index, option] of Array.from(control.options).entries()) option.selected = selected.has(index);
        }
      }
    }

    function restoreFocus(saved, capturedNodes) {
      if (!saved || typeof saved !== 'object') return;
      const element = resolveIdentity(saved.identity);
      if (!(element instanceof HTMLElement) || !matchesFormRelation(element, saved.form)) return;
      if (capturedNodes?.has(element)) return;
      if (element instanceof HTMLInputElement && ['password', 'file'].includes(element.type.toLowerCase())) return;
      try { element.focus({ preventScroll: true }); }
      catch (_) { return; }
      if (document.activeElement !== element || !saved.selection) return;
      if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)) return;
      const { start, end, direction } = saved.selection;
      if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start
          || end > element.value.length || ![null, 'forward', 'backward', 'none'].includes(direction)) return;
      try { element.setSelectionRange(start, end, direction); }
      catch (_) {}
    }

    function restoreMedia(media, capturedNodes) {
      if (!Array.isArray(media)) return;
      for (const saved of media.slice(0, 32)) {
        if (!saved || typeof saved !== 'object') continue;
        const element = resolveIdentity(saved.identity);
        if (!(element instanceof HTMLMediaElement) || mediaSource(element) !== saved.source) continue;
        if (capturedNodes?.has(element)) continue;
        if (!Number.isFinite(saved.currentTime) || typeof saved.paused !== 'boolean'
            || typeof saved.muted !== 'boolean' || !Number.isFinite(saved.volume)
            || saved.volume < 0 || saved.volume > 1 || !Number.isFinite(saved.playbackRate)
            || saved.playbackRate <= 0) continue;
        const apply = () => {
          if (!canRestore()) return;
          if (mediaSource(element) !== saved.source) return;
          const time = Number.isFinite(element.duration)
            ? Math.min(saved.currentTime, Math.max(0, element.duration))
            : saved.currentTime;
          try { element.currentTime = Math.max(0, time); } catch (_) {}
          element.muted = saved.muted;
          element.volume = saved.volume;
          try { element.playbackRate = saved.playbackRate; } catch (_) {}
          if (saved.paused) element.pause();
          else {
            try { element.play().catch(() => {}); } catch (_) {}
          }
        };
        if (element.readyState >= 1) apply();
        else element.addEventListener('loadedmetadata', apply, { once: true });
      }
    }

    function restore(saved, capturedNodes = null) {
      if (!canRestore()) return;
      restoreFormControls(saved?.controls, capturedNodes);
      restoreMedia(saved?.media, capturedNodes);
      restoreFocus(saved?.focus, capturedNodes);
    }

    function restoreScrollPosition(position) {
      if (!canRestore()) return;
      const root = document.documentElement;
      const previousBehavior = root.style.getPropertyValue('scroll-behavior');
      const previousPriority = root.style.getPropertyPriority('scroll-behavior');
      root.style.setProperty('scroll-behavior', 'auto', 'important');
      window.scrollTo({ left: position[0], top: position[1], behavior: 'instant' });
      if (root.style.getPropertyValue('scroll-behavior') === 'auto'
          && root.style.getPropertyPriority('scroll-behavior') === 'important') {
        if (previousBehavior) root.style.setProperty('scroll-behavior', previousBehavior, previousPriority);
        else root.style.removeProperty('scroll-behavior');
      }
      // An empty style attribute would read as a changed document element later.
      if (!root.getAttribute('style')) root.removeAttribute('style');
    }

    function saveReloadState() {
      if (!canRestore()) {
        try { sessionStorage.removeItem('tola:reload-position'); } catch (_) {}
        return;
      }
      let payload;
      try {
        payload = JSON.stringify({
          href: location.href,
          x: Number.isFinite(window.scrollX) ? window.scrollX : 0,
          y: Number.isFinite(window.scrollY) ? window.scrollY : 0,
          requestedAt: Date.now(),
          navigation: capture(),
        });
        if (payload.length > 512 * 1024) {
          payload = JSON.stringify({
            href: location.href,
            x: Number.isFinite(window.scrollX) ? window.scrollX : 0,
            y: Number.isFinite(window.scrollY) ? window.scrollY : 0,
            requestedAt: Date.now(),
          });
        }
        sessionStorage.setItem('tola:reload-position', payload);
      } catch (_) {
        // Storage may be disabled or unavailable in private browsing contexts.
      }
    }

    function restoreReloadState() {
      let saved;
      try {
        const value = sessionStorage.getItem('tola:reload-position');
        if (!value) return;
        sessionStorage.removeItem('tola:reload-position');
        saved = JSON.parse(value);
      } catch (_) {
        return;
      }
      if (!saved || saved.href !== location.href
          || !Number.isFinite(saved.x) || !Number.isFinite(saved.y)) return;
      const restoreSaved = () => {
        if (!canRestore()) return;
        restore(saved.navigation);
        restoreScrollPosition([saved.x, saved.y]);
        if (Number.isFinite(saved.requestedAt)) {
          mark('reload-latency-ms', Math.max(0, Date.now() - saved.requestedAt));
        }
        mark('reload-restored');
      };
      if (document.readyState === 'complete') {
        requestAnimationFrame(restoreSaved);
      } else {
        addEventListener('load', () => requestAnimationFrame(restoreSaved), { once: true });
      }
    }

    return {
      capture,
      capturePatch,
      restore,
      restoreScrollPosition,
      saveReloadState,
      restoreReloadState,
    };
  }

  function cssUrls(text, base, rewrite = null) {
    const urls = [];
    let unknown = !!text && (text.includes('\\') || /(?:-webkit-)?image-set\s*\(/iu.test(text));
    const rewritten = (text || '').replace(/url\(\s*(?:"([^"]*)"|'([^']*)'|([^)]*))\s*\)/giu,
      (match, doubleQuoted, singleQuoted, unquoted) => {
        const value = (doubleQuoted ?? singleQuoted ?? unquoted ?? '').trim();
        if (!value || value.startsWith('data:') || value.startsWith('#')) return match;
        try {
          const url = new URL(value, base).href;
          urls.push(url);
          const next = rewrite?.(url);
          return next ? `url(${JSON.stringify(next)})` : match;
        } catch (_) { unknown = true; return match; }
      });
    return { urls, unknown, text: rewritten };
  }

  /// A change is applied in place only from what the page actually fetched and
  /// from the references readable in the live DOM, so every observation is
  /// retained until the page navigates.
  function createResourceEvidence({ addresses }) {
    const observedPaths = new Set();
    const fetchedData = new Set();
    const fetchedOther = new Set();
    const fetchedUnsafe = new Set();
    let resourceObserver = null;
    let complete = false;
    let behaviorObserver = null;
    let behaviorObserved = false;

    function record(entries) {
      for (const entry of entries) {
        const path = addresses.outputPath(entry.name);
        if (path === null) continue;
        if (!observedPaths.has(path) && observedPaths.size >= 16384) {
          complete = false;
          break;
        }
        observedPaths.add(path);
        if (['fetch', 'xmlhttprequest'].includes(entry.initiatorType)) fetchedData.add(path);
        else {
          fetchedOther.add(path);
          if (!['img', 'css', 'link', 'video'].includes(entry.initiatorType)) fetchedUnsafe.add(path);
        }
      }
    }

    /// A hyperlink destination is resolved when the user follows it, so a changed
    /// target never makes this document's rendering depend on the target's bytes.
    function isNavigationReference(element, attribute) {
      return (attribute === 'href' || attribute === 'xlink:href')
        && (element.localName === 'a' || element.localName === 'area');
    }

    function recordBehavior(records) {
      if (behaviorObserved) return;
      for (const mutation of records) {
        if (mutation.type === 'attributes') {
          const element = mutation.target;
          if (element.closest(RUNTIME_CONTAINER_SELECTOR)) continue;
          if (activeAttribute(element, mutation.attributeName, mutation.oldValue)
              || (element.localName === 'script' && mutation.attributeName === 'type'
                && activeScript(element, mutation.oldValue))
              || (element.localName === 'script' && mutation.attributeName === 'src' && mutation.oldValue !== null)
              || siteBehavior(element)) {
            behaviorObserved = true;
            return;
          }
        }
        for (const node of [...mutation.addedNodes, ...mutation.removedNodes]) {
          if (node instanceof Element && siteBehavior(node)) {
            behaviorObserved = true;
            return;
          }
        }
      }
    }

    function observe() {
      behaviorObserved = siteBehavior(document);
      try {
        behaviorObserver = new MutationObserver(recordBehavior);
        behaviorObserver.observe(document, {
          childList: true, subtree: true, attributes: true, attributeOldValue: true,
        });
        if (typeof PerformanceObserver !== 'function') return;
        const buffered = performance.getEntriesByType('resource');
        // Earlier user scripts may have cleared the timing buffer.
        complete = buffered.length < 250;
        record(buffered);
        // Live observers receive entries before the browser's timeline capacity is checked.
        resourceObserver = new PerformanceObserver(entries => record(entries.getEntries()));
        resourceObserver.observe({ type: 'resource', buffered: true });
      } catch (_) {
        behaviorObserved = true;
        complete = false;
      }
    }

    function drain() {
      if (resourceObserver) record(resourceObserver.takeRecords());
      if (behaviorObserver) recordBehavior(behaviorObserver.takeRecords());
    }

    function dependencies() {
      const paths = new Map();
      let unknown = false;
      const addPath = (path, consume) => {
        let consumers = paths.get(path);
        if (!consumers) {
            consumers = { stylesheets: [], cssImages: [], media: [], unsupported: false,
              dataFetched: false, otherFetched: false, unsafeFetched: false };
          paths.set(path, consumers);
        }
        consume(consumers);
      };
      const add = (url, consume) => {
        const path = addresses.outputPath(url);
        if (path !== null) addPath(path, consume);
      };
      const unsupported = consumers => { consumers.unsupported = true; };
      const cssText = (text, base, consume = unsupported) => {
        const references = cssUrls(text, base);
        if (references.unknown) unknown = true;
        for (const url of references.urls) add(url, consume);
      };
      for (const element of document.querySelectorAll('*')) {
        if (element.matches(RUNTIME_DOM_SELECTOR) || element.closest(RUNTIME_CONTAINER_SELECTOR)) continue;
        const stylesheet = element.matches('link[rel~="stylesheet"][href]:not([data-tola-staging])');
        for (const attribute of ['src', 'href', 'poster', 'data', 'xlink:href']) {
          const value = element.getAttribute(attribute);
          if (!value) continue;
          if (stylesheet && attribute === 'href') {
            add(value, consumers => consumers.stylesheets.push(element));
          } else if ((element.tagName === 'IMG' && attribute === 'src')
              || (element.tagName === 'VIDEO' && attribute === 'poster')) {
            add(value, consumers => consumers.media.push({ element, attribute, value }));
          } else if (!isNavigationReference(element, attribute)) {
            add(value, unsupported);
          }
        }
        const srcset = element.getAttribute('srcset');
        if (srcset) {
          let position = 0;
          while (position < srcset.length) {
            while (position < srcset.length && /[\t\n\f\r ,]/u.test(srcset[position])) position += 1;
            const start = position;
            while (position < srcset.length && !/[\t\n\f\r ]/u.test(srcset[position])) position += 1;
            let url = srcset.slice(start, position);
            if (url.endsWith(',')) {
              url = url.replace(/,+$/u, '');
            } else {
              // Commas inside a URL (including data URLs) are not separators.
              // Only a descriptor-ending comma starts the next candidate.
              let parentheses = 0;
              while (position < srcset.length) {
                const character = srcset[position++];
                if (character === '(') parentheses += 1;
                else if (character === ')') parentheses = Math.max(0, parentheses - 1);
                else if (character === ',' && parentheses === 0) break;
              }
            }
            if (url) add(url, unsupported);
          }
        }
        if (element.currentSrc && !(element.tagName === 'IMG' && !srcset
            && element.currentSrc === element.src)) add(element.currentSrc, unsupported);
        cssText(element.getAttribute('style'), document.baseURI);
      }
      const visited = new Set();
      const visitRules = (rules, base, link) => {
        for (const rule of rules) {
          if (rule.styleSheet) {
            if (rule.href) add(new URL(rule.href, base).href, unsupported);
            visit(rule.styleSheet, link);
          } else {
            // Scan declarations to avoid escaped selectors and repeated nested CSS.
            if (rule.style) cssText(rule.style.cssText, base, link && rule.type !== CSSRule.FONT_FACE_RULE
              ? consumers => { if (!consumers.cssImages.includes(link)) consumers.cssImages.push(link); }
              : unsupported);
            else if (!rule.cssRules) cssText(rule.cssText, base);
            if (rule.cssRules) visitRules(rule.cssRules, base, link);
          }
        }
      };
      const visit = (sheet, owner = sheet?.ownerNode) => {
        if (owner instanceof Element && owner.matches(RUNTIME_DOM_SELECTOR)) return;
        if (!sheet || visited.has(sheet)) return;
        visited.add(sheet);
        let rules;
        try { rules = sheet.cssRules; }
        catch (_) { unknown = true; return; }
        const base = sheet.href || document.baseURI;
        const link = owner instanceof HTMLLinkElement && owner.matches('link[rel~="stylesheet"][href]:not([data-tola-staging])')
          ? owner : null;
        visitRules(rules, base, link);
      };
      for (const sheet of document.styleSheets) visit(sheet);
      for (const sheet of document.adoptedStyleSheets || []) visit(sheet);
      for (const path of observedPaths) addPath(path, consumers => {
        consumers.dataFetched = fetchedData.has(path);
        consumers.otherFetched = fetchedOther.has(path);
        consumers.unsafeFetched = fetchedUnsafe.has(path);
      });
      return { paths, unknown };
    }

    return {
      observe,
      drain,
      dependencies,
      get complete() { return complete; },
      get hasSiteBehavior() {
        if (behaviorObserver) recordBehavior(behaviorObserver.takeRecords());
        if (!behaviorObserved) behaviorObserved = siteBehavior(document);
        return behaviorObserved;
      },
    };
  }

  function imageRequest(source) {
    const image = new Image();
    for (const name of ['crossorigin', 'referrerpolicy']) {
      if (!(source instanceof HTMLImageElement) && (name === 'referrerpolicy' || !(source instanceof HTMLVideoElement))) continue;
      const value = source.getAttribute(name);
      if (value !== null) image.setAttribute(name, value);
    }
    return image;
  }

  function prepareImage(image, url, description, signal = null) {
    return new Promise((resolve, reject) => {
      let settled = false;
      const finish = error => {
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        image.removeEventListener('load', loaded);
        image.removeEventListener('error', failed);
        signal?.removeEventListener('abort', aborted);
        if (error) {
          image.removeAttribute('src');
          reject(error);
        } else {
          resolve();
        }
      };
      const loaded = async () => {
        try {
          await image.decode();
          finish();
        } catch (_) { failed(); }
      };
      const failed = () => finish(new Error(`${description} could not be loaded`));
      const aborted = () => finish(signal.reason || new Error(`${description} could not be loaded`));
      const timeout = setTimeout(() => finish(new Error(`${description} did not load in time`)), 5000);
      image.addEventListener('load', loaded);
      image.addEventListener('error', failed);
      signal?.addEventListener('abort', aborted, { once: true });
      if (signal?.aborted) { aborted(); return; }
      image.src = url;
    });
  }

  function createResourceReplacements({ addresses }) {
    const MAX_STYLESHEET_IMAGES = 128;
    function stylesheetImages(sheet, consume, fonts = () => {}, visited = new Set()) {
      if (!sheet || visited.has(sheet)) return;
      visited.add(sheet);
      const base = sheet.href || document.baseURI;
      const visit = rules => {
        for (const rule of rules) {
          if (rule.styleSheet) stylesheetImages(rule.styleSheet, consume, fonts, visited);
          else {
            if (rule.style && rule.type !== CSSRule.FONT_FACE_RULE) {
              for (const name of ['filter', 'clip-path', 'offset-path', '-webkit-filter', '-webkit-clip-path']) {
                if (cssUrls(rule.style.getPropertyValue(name), base).urls.length) {
                  throw new Error('the stylesheet contains a resource that could not be prepared');
                }
              }
              consume(rule.style, base);
            }
            if (rule.type === CSSRule.FONT_FACE_RULE) fonts(rule.cssText, base, rule.style);
            if (rule.cssRules) visit(rule.cssRules);
            else if (!rule.style && cssUrls(rule.cssText, base).urls.length) {
              throw new Error('the stylesheet contains a resource that could not be prepared');
            }
          }
        }
      };
      visit(sheet.cssRules);
    }

    async function prepareStylesheetImages(sheet, link, representations, description, signal) {
      const inherited = new Map();
      const fonts = new Set();
      stylesheetImages(link.sheet, (style, base) => {
        for (const url of cssUrls(style.cssText, base).urls) {
          const path = addresses.outputPath(url);
          const representation = new URL(url).searchParams.get('tola-representation');
          if (path !== null && addresses.isHexIdentity(representation)) inherited.set(path, representation);
        }
      }, (text, base) => fonts.add(cssUrls(text, base, url => url).text));
      for (const [path, representation] of representations) inherited.set(path, representation);
      const urls = new Set();
      stylesheetImages(sheet, (style, base) => {
        const references = cssUrls(style.cssText, base, url => {
          const representation = addresses.representationOf(inherited, url);
          return representation ? addresses.withRepresentation(url, representation) : null;
        });
        if (references.unknown) throw new Error(`the images in ${description} could not be prepared`);
        if (references.text !== style.cssText) style.cssText = references.text;
        for (const url of cssUrls(style.cssText, base).urls) {
          urls.add(url);
          if (urls.size > MAX_STYLESHEET_IMAGES) throw new Error(`the images in ${description} could not be prepared`);
        }
      }, (text, base, style) => {
        const family = style.getPropertyValue('font-family').replace(/^["']|["']$/g, '');
        const available = Array.from(document.fonts).filter(font => font.family.replace(/^["']|["']$/g, '') === family);
        if (!fonts.has(cssUrls(text, base, url => url).text) || !available.length
            || available.some(font => font.status !== 'loaded')) {
          throw new Error(`the fonts in ${description} could not be prepared`);
        }
      });
      const images = Array.from(urls, () => new Image());
      const imageUrls = Array.from(urls);
      let next = 0;
      let failure = null;
      await Promise.all(Array.from({ length: Math.min(4, images.length) }, async () => {
        while (!failure && !signal.aborted && next < images.length) {
          const index = next++;
          try { await prepareImage(images[index], imageUrls[index], `an image in ${description}`, signal); }
          catch (error) { failure ||= error; }
        }
      }));
      if (failure) throw failure;
      if (signal.aborted) throw signal.reason;
      return images;
    }

    function prepareStylesheet(path, representation, references, representations) {
      const staged = [];
      const loads = [];
      references.forEach(link => {
        const replacement = link.cloneNode(true);
        const markup = link.outerHTML;
        const parent = link.parentNode;
        const disabled = link.disabled;
        const controller = new AbortController();
        const targetMedia = replacement.getAttribute('media');
        replacement.dataset.tolaStaging = '';
        replacement.setAttribute('media', 'not all');
        replacement.href = representation ? addresses.withRepresentation(link.href, representation) : link.href;
        const stagedLink = { link, replacement, targetMedia, markup, parent, disabled, controller, images: [], timeout: null };
        staged.push(stagedLink);
        loads.push(new Promise((resolve, reject) => {
          const timeout = setTimeout(() => {
            controller.abort(new Error(`the stylesheet \`${path}\` did not load in time`));
            replacement.remove();
            reject(new Error(`the stylesheet \`${path}\` did not load in time`));
          }, 5000);
          replacement.onload = async () => {
            try {
              stagedLink.images = await prepareStylesheetImages(replacement.sheet, link, representations, `the stylesheet \`${path}\``, controller.signal);
              clearTimeout(timeout);
              resolve();
            } catch (error) {
              clearTimeout(timeout);
              replacement.remove();
              reject(error);
            }
          };
          replacement.onerror = () => {
            clearTimeout(timeout);
            replacement.remove();
            reject(new Error(`the stylesheet \`${path}\` could not be loaded`));
          };
          stagedLink.timeout = timeout;
        }));
        link.parentNode.insertBefore(replacement, link.nextSibling);
      });
      const dispose = () => {
        for (const item of staged) {
          clearTimeout(item.timeout);
          item.controller.abort();
          item.replacement.onload = null;
          item.replacement.onerror = null;
          item.replacement.remove();
        }
      };
      if (staged.length === 0) {
        return Promise.reject(new Error(`the page no longer references \`${path}\``));
      }
      return Promise.all(loads).then(() => {
        let committed = false;
        return {
          validate: () => {
            for (const item of staged) {
              if (!item.link.isConnected || item.link.parentNode !== item.parent
                  || item.link.outerHTML !== item.markup || item.link.disabled !== item.disabled
                  || item.link.nextSibling !== item.replacement) {
                throw new Error(`the page changed how it loads \`${path}\``);
              }
            }
          },
          commit: () => {
            if (committed) return;
            committed = true;
            for (const item of staged) {
              delete item.replacement.dataset.tolaStaging;
              item.replacement.disabled = item.disabled;
              if (item.targetMedia === null) item.replacement.removeAttribute('media');
              else item.replacement.setAttribute('media', item.targetMedia);
            }
            for (const item of staged) item.link.remove();
          },
          dispose,
        };
      }).catch(error => {
        dispose();
        throw error;
      });
    }

    function prepareMedia(path, representation, references) {
      const changes = references.map(reference => ({
        ...reference,
        next: addresses.withRepresentation(reference.value, representation),
        image: imageRequest(reference.element),
        attributes: ['crossorigin', 'referrerpolicy', 'srcset', 'sizes', 'loading']
          .map(name => [name, reference.element.getAttribute(name)]),
      }));
      if (changes.length === 0) {
        return Promise.reject(new Error(`the page no longer references \`${path}\``));
      }
      const load = change => prepareImage(change.image, change.next, `the file \`${path}\``);
      return Promise.all(changes.map(load)).then(() => ({
        validate: () => {
          for (const change of changes) {
            if (!change.element.isConnected
                || change.element.getAttribute(change.attribute) !== change.value
                || change.attributes.some(([name, value]) => change.element.getAttribute(name) !== value)) {
              throw new Error(`the page changed how it loads \`${path}\``);
            }
          }
        },
        commit: () => {
          for (const change of changes) change.element.setAttribute(change.attribute, change.next);
        },
        dispose: () => {},
      }));
    }

    return { prepareImage, prepareStylesheet, prepareMedia };
  }

  function createUpdateCallbacks(diff, outputs, dependencies, pageActive) {
    const MAX_UPDATE_CALLBACKS = 32;
    const changes = Object.freeze(outputs.map((output, index) => Object.freeze({
      path: output.path, operation: diff.changes[index].operation, kind: output.kind,
    })));
    const available = new Map(changes.map(change => [change.path, change]));
    const accepted = new Set();
    const callbacks = [];
    let open = true;
    let failure = null;
    const refuse = () => {
      const error = new Error('the page could not accept this site update');
      if (open) failure = error;
      throw error;
    };
    const accept = (paths, callback) => {
      if (!open || !Array.isArray(paths) || !paths.length || typeof callback !== 'function'
          || callbacks.length >= MAX_UPDATE_CALLBACKS) return refuse();
      const pathsAccepted = new Set();
      for (const path of paths) {
        const change = available.get(path);
        const consumers = dependencies.paths.get(path);
        const builtIn = consumers && (consumers.stylesheets.length || consumers.cssImages.length || consumers.media.length);
        if (!change || change.kind === 'html-document' || accepted.has(path) || pathsAccepted.has(path)
            || consumers?.unsupported || consumers?.unsafeFetched
            || (consumers && !builtIn && (!consumers.dataFetched || consumers.otherFetched))) return refuse();
        pathsAccepted.add(path);
      }
      for (const path of pathsAccepted) accepted.add(path);
      callbacks.push(callback);
    };
    try {
      dispatchEvent(new CustomEvent('tola:before-update', {
        detail: Object.freeze({ from: diff.from, to: diff.to, changes, accept }),
      }));
    } finally { open = false; }
    if (failure) throw failure;
    return {
      accepts: path => accepted.has(path),
      run: async () => {
        if (!callbacks.length) return;
        const controller = new AbortController();
        const cancel = () => controller.abort(new Error('the page stopped accepting site updates'));
        const timeout = setTimeout(() => controller.abort(new Error('the page did not finish its site update in time')), 5000);
        addEventListener('pagehide', cancel, { once: true });
        const aborted = new Promise((_, reject) => {
          controller.signal.addEventListener('abort', () => reject(controller.signal.reason), { once: true });
        });
        try {
          if (!pageActive()) cancel();
          await Promise.race([
            Promise.all(callbacks.map(callback => Promise.resolve().then(() => callback({ signal: controller.signal })))),
            aborted,
          ]);
          if (controller.signal.aborted) throw controller.signal.reason;
        } catch (error) {
          controller.abort(error);
          throw error;
        } finally {
          clearTimeout(timeout);
          removeEventListener('pagehide', cancel);
        }
      },
    };
  }

  /// A patch is prepared only when every part of it can be proven safe;
  /// otherwise the caller navigates. Unchanged nodes keep their identity, so
  /// scroll position, focus, form values, and media state survive.
  function createDocumentPatch({ addresses, pageState, replacements }) {
    const PATCHED_HEAD_ELEMENTS = new Set(['title', 'meta']);
    const URL_ATTRIBUTES = ['href', 'src', 'poster', 'data'];
    let sourceDocument = null;
    let sourceRevision = null;
    let sourceLoading = Promise.resolve();
    let sourceFailure = null;

    function contentNodes(parent) {
      return Array.from(parent.childNodes).filter(node => !(
        node.nodeType === 1 && node.matches(RUNTIME_DOM_SELECTOR)
      ));
    }

    function attributeValue(element, name) {
      const value = element.getAttribute(name);
      if (value === null) return null;
      return URL_ATTRIBUTES.includes(name) ? withoutRepresentation(value) : value;
    }

    function sameAttributes(left, right, ignored = null) {
      const names = new Set([...left.attributes].map(attribute => attribute.name)
        .concat([...right.attributes].map(attribute => attribute.name)));
      for (const name of names) {
        if (name === ignored) continue;
        if (attributeValue(left, name) !== attributeValue(right, name)) return false;
      }
      return true;
    }

    function sameMarkup(left, right) {
      if (left.nodeType !== right.nodeType) return false;
      if (left.nodeType !== 1) return left.nodeValue === right.nodeValue;
      if (left.localName !== right.localName) return false;
      if (!sameAttributes(left, right)) return false;
      const leftChildren = contentNodes(left);
      const rightChildren = contentNodes(right);
      return leftChildren.length === rightChildren.length
        && leftChildren.every((child, index) => sameMarkup(child, rightChildren[index]));
    }

    function requireReplaceableAttribute(element, name, value) {
      const tag = element.localName;
      if (name === 'srcset' || name === 'xlink:href' || name === 'poster' || name === 'data') {
        throw new Error(`the \`${tag}\` \`${name}\` attribute could not be updated in place`);
      }
      if (name === 'style' && /url\s*\(/i.test(value || '')) {
        throw new Error(`the \`${tag}\` \`${name}\` attribute could not be updated in place`);
      }
      if (name === 'href' && ['link', 'use', 'image'].includes(tag)) {
        throw new Error(`the \`${tag}\` \`${name}\` attribute could not be updated in place`);
      }
      if (name === 'src' && tag !== 'img') {
        throw new Error(`the \`${tag}\` \`${name}\` attribute could not be updated in place`);
      }
    }

    function createPlan() {
      const plan = { structural: false, head: [], body: [], images: [] };
      plan.text = (node, value) => plan.body.push({ op: 'text', node, value });
      plan.attributes = (node, set, remove) => plan.body.push({ op: 'attributes', node, set, remove });
      plan.remove = node => {
        plan.structural = true;
        plan.body.push({ op: 'remove', node });
      };
      plan.replace = (node, replacement) => {
        plan.structural = true;
        plan.body.push({ op: 'replace', node, replacement });
      };
      plan.insert = (parent, node) => {
        plan.structural = true;
        plan.body.push({ op: 'insert', parent, node });
      };
      return plan;
    }

    function imageUrl(representations, value) {
      const representation = addresses.representationOf(representations, value);
      return representation ? addresses.withRepresentation(value, representation) : value;
    }

    /// Prepare one image address. `detached` marks a copy the patch will insert,
    /// which may carry the address itself; a live element keeps its own.
    function planImage(plan, representations, image, detached) {
      const source = image.getAttribute('src');
      if (!source) return null;
      const url = imageUrl(representations, source);
      if (detached) image.setAttribute('src', url);
      plan.images.push({
        node: detached ? image : imageRequest(image),
        url,
        description: `the image \`${source}\``,
      });
      return url;
    }

    async function prepareImages(plan) {
      await Promise.all(plan.images.map(async ({ node, url, description }) => {
        const originalLoading = node.getAttribute('loading');
        node.setAttribute('loading', 'eager');
        try {
          await prepareImage(node, url, description);
        } finally {
          if (originalLoading === null) node.removeAttribute('loading');
          else node.setAttribute('loading', originalLoading);
        }
      }));
    }

    function prepareReplacement(plan, representations, nextNode) {
      const offending = activeContent(nextNode);
      if (offending) {
        throw new Error(`the updated page contains \`${offending.localName}\`, which cannot be updated in place`);
      }
      const replacement = document.importNode(nextNode, true);
      const images = replacement.nodeType === 1
        ? [replacement, ...replacement.querySelectorAll('img[src]')].filter(node => node.localName === 'img')
        : [];
      for (const image of images) planImage(plan, representations, image, true);
      return replacement;
    }

    function planAttributes(plan, representations, currentNode, nextNode) {
      const set = [];
      const remove = [];
      for (const attribute of Array.from(currentNode.attributes)) {
        if (nextNode.hasAttribute(attribute.name)) continue;
        requireReplaceableAttribute(currentNode, attribute.name, '');
        remove.push(attribute.name);
      }
      for (const attribute of Array.from(nextNode.attributes)) {
        const value = nextNode.getAttribute(attribute.name);
        if (attributeValue(currentNode, attribute.name) === attributeValue(nextNode, attribute.name)) continue;
        requireReplaceableAttribute(currentNode, attribute.name, value);
        set.push([attribute.name, value]);
      }
      // An image source keeps its revision identity, so it is prepared first.
      const source = set.findIndex(([name]) => name === 'src');
      if (source !== -1 && currentNode.localName === 'img') {
        set[source] = ['src', planImage(plan, representations, nextNode, false)];
      }
      if (set.length || remove.length) plan.attributes(currentNode, set, remove);
    }

    function planNode(plan, representations, currentNode, nextNode) {
      if (currentNode.nodeType !== nextNode.nodeType) {
        plan.replace(currentNode, prepareReplacement(plan, representations, nextNode));
        return;
      }
      if (currentNode.nodeType !== 1) {
        if (currentNode.nodeValue !== nextNode.nodeValue) plan.text(currentNode, nextNode.nodeValue);
        return;
      }
      if (currentNode.localName !== nextNode.localName) {
        plan.replace(currentNode, prepareReplacement(plan, representations, nextNode));
        return;
      }
      planAttributes(plan, representations, currentNode, nextNode);
      planChildren(plan, representations, currentNode, nextNode);
    }

    function planChildren(plan, representations, currentParent, nextParent) {
      const currentNodes = contentNodes(currentParent);
      const nextNodes = contentNodes(nextParent);
      const shared = Math.min(currentNodes.length, nextNodes.length);
      for (let index = 0; index < shared; index += 1) {
        planNode(plan, representations, currentNodes[index], nextNodes[index]);
      }
      for (let index = shared; index < currentNodes.length; index += 1) {
        plan.remove(currentNodes[index]);
      }
      for (let index = shared; index < nextNodes.length; index += 1) {
        plan.insert(currentParent, prepareReplacement(plan, representations, nextNodes[index]));
      }
    }

    function planHead(plan, representations, currentHead, nextHead) {
      const currentNodes = contentNodes(currentHead);
      const nextNodes = contentNodes(nextHead);
      if (currentNodes.length !== nextNodes.length) throw new Error('the update could not be applied in place');
      const bodyBefore = plan.body.length;
      for (let index = 0; index < currentNodes.length; index += 1) {
        const currentNode = currentNodes[index];
        const nextNode = nextNodes[index];
        const patchable = currentNode.nodeType === 1 && nextNode.nodeType === 1
          && currentNode.localName === nextNode.localName
          && PATCHED_HEAD_ELEMENTS.has(currentNode.localName);
        if (!patchable) {
          if (!sameMarkup(currentNode, nextNode)) throw new Error('the update could not be applied in place');
          continue;
        }
        planNode(plan, representations, currentNode, nextNode);
      }
      // The head holds nothing the patch may restructure.
      if (plan.structural) throw new Error('the update could not be applied in place');
      plan.head.push(...plan.body.splice(bodyBefore));
    }

    function applyPatch(plan) {
      const apply = patch => {
        switch (patch.op) {
          case 'text': patch.node.nodeValue = patch.value; break;
          case 'attributes':
            for (const [name, value] of patch.set) patch.node.setAttribute(name, value);
            for (const name of patch.remove) patch.node.removeAttribute(name);
            break;
          case 'remove': patch.node.remove(); break;
          case 'replace': patch.node.replaceWith(patch.replacement); break;
          case 'insert': patch.parent.appendChild(patch.node); break;
        }
      };
      for (const patch of plan.head) apply(patch);
      for (const patch of plan.body) apply(patch);
    }

    async function loadDocument(revision, representation = null) {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 5000);
      let next;
      try {
        const url = representation ? addresses.withRepresentation(location.href, representation) : location.href;
        const response = await fetch(url, {
          headers: { 'X-Tola-Hot-Reload': 'true', 'X-Tola-Revision': revision },
          cache: 'no-store', redirect: 'error', signal: controller.signal,
        });
        if (!response.ok || !(response.headers.get('content-type') || '').startsWith('text/html')) {
          throw new Error(`the updated page could not be loaded (HTTP ${response.status})`);
        }
        next = new DOMParser().parseFromString(await response.text(), 'text/html');
      } finally {
        clearTimeout(timeout);
      }
      return next;
    }

    function initialize(revision) {
      sourceRevision = revision;
      sourceLoading = loadDocument(revision).then(next => {
        sourceDocument = next;
      }, error => { sourceFailure = error; });
    }

    function advance(from, to, next) {
      if (sourceRevision !== from) return;
      sourceRevision = to;
      if (next) sourceDocument = next;
    }

    async function prepareClasses(output, diff) {
      await sourceLoading;
      if (!sourceDocument || sourceRevision !== diff.from) {
        throw sourceFailure || new Error('the previous page could not be compared with this update');
      }
      const next = await loadDocument(diff.to, output.representation);
      const changes = [];
      const compare = (before, after, path) => {
        if (before.nodeType !== after.nodeType) throw new Error('the updated page requires navigation');
        if (before.nodeType !== 1) {
          if (before.nodeValue !== after.nodeValue) throw new Error('the updated page requires navigation');
          return;
        }
        if (before.namespaceURI !== after.namespaceURI || before.localName !== after.localName
            || !sameAttributes(before, after, 'class')) throw new Error('the updated page requires navigation');
        const previous = before.getAttribute('class');
        const value = after.getAttribute('class');
        if (previous !== value) {
          if (before.namespaceURI !== 'http://www.w3.org/1999/xhtml'
              || before.localName.includes('-') || before.hasAttribute('is')
              || ['script', 'style', 'link', 'iframe', 'object', 'embed'].includes(before.localName)) {
            throw new Error('the updated page requires navigation');
          }
          changes.push({ before, path, previous, value });
        }
        const beforeChildren = contentNodes(before);
        const afterChildren = contentNodes(after);
        if (beforeChildren.length !== afterChildren.length) throw new Error('the updated page requires navigation');
        beforeChildren.forEach((child, index) => compare(child, afterChildren[index], [...path, index]));
      };
      compare(sourceDocument.documentElement, next.documentElement, []);

      const staticIdentity = (before, live) => {
        if (before.nodeType !== live.nodeType) return false;
        if (before.nodeType !== 1) return before.nodeValue === live.nodeValue;
        if (before.namespaceURI !== live.namespaceURI || before.localName !== live.localName) return false;
        if (before.id && before.id.length <= 1024) {
          const selector = `#${CSS.escape(before.id)}`;
          return before.id === live.id && sourceDocument.querySelectorAll(selector).length === 1
            && document.querySelectorAll(selector).length === 1;
        }
        if (!sameAttributes(before, live, 'class')) return false;
        const beforeChildren = contentNodes(before);
        const liveChildren = contentNodes(live);
        return beforeChildren.length === liveChildren.length
          && beforeChildren.every((child, index) => staticIdentity(child, liveChildren[index]));
      };
      const resolve = change => {
        const source = change.before;
        if (source.id && source.id.length <= 1024) {
          const selector = `#${CSS.escape(source.id)}`;
          const sourceMatches = sourceDocument.querySelectorAll(selector);
          const liveMatches = document.querySelectorAll(selector);
          if (sourceMatches.length === 1 && liveMatches.length === 1
              && liveMatches[0].namespaceURI === source.namespaceURI
              && liveMatches[0].localName === source.localName) return liveMatches[0];
          throw new Error('the updated class no longer identifies one page element');
        }
        let live = document.documentElement;
        let before = sourceDocument.documentElement;
        for (const index of change.path) {
          const beforeChildren = contentNodes(before);
          const liveChildren = contentNodes(live);
          const selected = beforeChildren[index];
          if (beforeChildren.length !== liveChildren.length
              || beforeChildren.some((child, position) => !staticIdentity(child, liveChildren[position]))
              || liveChildren.filter(child => staticIdentity(selected, child)
                && (child.nodeType !== 1 || child.getAttribute('class') === selected.getAttribute('class'))).length !== 1) {
            throw new Error('the updated class no longer identifies one page element');
          }
          before = beforeChildren[index];
          live = liveChildren[index];
        }
        return live;
      };
      const classes = changes.map(change => ({ ...change, node: resolve(change) }));
      return {
        source: next,
        scripted: true,
        classesChanged: classes.length > 0,
        validate: () => {
          for (const change of classes) {
            if (!change.node.isConnected || resolve(change) !== change.node
                || change.node.getAttribute('class') !== change.previous) {
              throw new Error('the page changed before its class update was applied');
            }
          }
        },
        commit: () => {
          for (const change of classes) {
            if (change.value === null) change.node.removeAttribute('class');
            else change.node.setAttribute('class', change.value);
          }
        },
      };
    }

    async function prepare(output, diff) {
      const representations = new Map(diff.changes
        .filter(change => change.operation !== 'removed')
        .map(change => addresses.validatedOutput(change))
        .filter(Boolean)
        .map(changed => [changed.path, changed.representation]));
      const next = await loadDocument(diff.to, output.representation);
      if (siteBehavior(next)) throw new Error('the updated page requires navigation');
      if (!sameAttributes(document.documentElement, next.documentElement)) {
        throw new Error('the update could not be applied in place');
      }
      const currentBody = document.body;
      const nextBody = next.body;
      if (!currentBody || !nextBody) throw new Error('the update could not be applied in place');
      if (!sameAttributes(currentBody, nextBody)) throw new Error('the update could not be applied in place');
      const signature = normalizedMarkup(document);
      const plan = createPlan();
      planHead(plan, representations, document.head, next.head);
      planChildren(plan, representations, currentBody, nextBody);
      await prepareImages(plan);
      return {
        source: next,
        classesChanged: plan.body.some(change => change.op === 'attributes'
          && (change.set.some(([name]) => name === 'class') || change.remove.includes('class'))),
        validate: () => {
          if (normalizedMarkup(document) !== signature) {
            throw new Error('the page changed before the update was applied');
          }
        },
        commit: () => {
          const { navigation, capturedNodes } = pageState.capturePatch();
          applyPatch(plan);
          pageState.restore(navigation, capturedNodes);
        },
      };
    }

    return { initialize, advance, prepare, prepareClasses };
  }

  /// The bottom-right indicator: whether a rebuild is running and how many errors and warnings
  /// the latest round reported. Diagnostic text stays in the terminal, so the page shows counts.
  function createDevStatus() {
    /// A rebuild appears only once it outlasts this delay; reported counts appear at once.
    const REVEAL_DELAY = 120;

    let status = { rebuilding: false, errors: 0, warnings: 0 };
    let awaitingPublication = false;
    let revealTimer = null;
    let parts = null;

    function setStatus(next, awaiting) {
      status = next;
      awaitingPublication = awaiting;
      render();
    }

    function render() {
      const reported = status.errors > 0 || status.warnings > 0;
      if (!status.rebuilding && !reported) {
        hide();
        return;
      }
      if (!parts && !reported) {
        if (!revealTimer) revealTimer = setTimeout(reveal, REVEAL_DELAY);
        return;
      }
      clearReveal();
      show();
    }

    function reveal() {
      revealTimer = null;
      if (status.rebuilding && !parts) show();
    }

    function show() {
      if (!parts) parts = createParts();
      parts.spinner.hidden = !status.rebuilding;
      parts.label.hidden = !status.rebuilding;
      parts.label.textContent = awaitingPublication ? 'Building site' : 'Rebuilding site';
      setCount(parts.errors, status.errors);
      setCount(parts.warnings, status.warnings);
      parts.root.setAttribute('aria-label', describe());
    }

    function hide() {
      clearReveal();
      parts?.root.remove();
      parts = null;
    }

    function clearReveal() {
      if (!revealTimer) return;
      clearTimeout(revealTimer);
      revealTimer = null;
    }

    function createParts() {
      injectRuntimeStyle('data-tola-dev-status', DEV_STATUS_CSS);
      const root = createElement('aside', 'tola-dev-status');
      root.id = 'tola-dev-status';
      root.setAttribute('role', 'status');
      root.setAttribute('aria-live', 'polite');
      const spinner = createElement('span', 'tola-dev-status-spinner');
      spinner.setAttribute('aria-hidden', 'true');
      const label = createElement('span', 'tola-dev-status-label');
      const errors = createCount('error');
      const warnings = createCount('warning');
      root.append(spinner, label, errors.badge, warnings.badge);
      (document.body || document.documentElement).appendChild(root);
      return { root, spinner, label, errors, warnings };
    }

    /// One severity badge and the number span a status update writes.
    function createCount(severity) {
      const badge = createElement('span', 'tola-dev-status-count');
      badge.dataset.severity = severity;
      const value = createElement('span', 'tola-dev-status-value', '0');
      badge.append(severityIcon(severity), value);
      return { badge, value };
    }

    /// A cross for errors, a triangle for warnings; the badge names the severity it stands for.
    function severityIcon(severity) {
      const namespace = 'http://www.w3.org/2000/svg';
      const icon = document.createElementNS(namespace, 'svg');
      icon.setAttribute('class', 'tola-dev-status-icon');
      icon.setAttribute('viewBox', '0 0 12 12');
      icon.setAttribute('aria-hidden', 'true');
      const path = document.createElementNS(namespace, 'path');
      path.setAttribute('d', severity === 'error'
        ? 'M3.6 3.6 8.4 8.4M8.4 3.6 3.6 8.4'
        : 'M6 1.6 11 10.4H1ZM6 4.9V7.3M6 9.4v.01');
      icon.appendChild(path);
      return icon;
    }

    function setCount(count, value) {
      count.badge.hidden = value === 0;
      count.value.textContent = String(value);
    }

    function describe() {
      const phrases = [];
      if (status.rebuilding) phrases.push(awaitingPublication ? 'Building the site' : 'Rebuilding the site');
      if (status.errors) phrases.push(`${status.errors} ${status.errors === 1 ? 'error' : 'errors'}`);
      if (status.warnings) phrases.push(`${status.warnings} ${status.warnings === 1 ? 'warning' : 'warnings'}`);
      return phrases.join(', ');
    }

    return { setStatus };
  }

  function createReloadTransport({ onMessage, onSocketChange, requestReload, requireReload, pageActive, reloadRequired }) {
    let port = null;
    let session = null;
    let generation = null;
    /// The live connection, or null. Every callback re-checks it, so a socket the transport has
    /// already relinquished cannot disturb the connection that replaced it.
    let socket = null;
    let reconnectTimer = null;
    let reconnectRetries = 0;

    function isReloadGeneration(value) {
      return typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);
    }

    function connect(nextPort) {
      if (typeof nextPort === 'number') port = nextPort;
      if (!port || !session || socket || reloadRequired() || !pageActive()) return;

      const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
      const host = location.hostname || 'localhost';
      const opening = new WebSocket(`${scheme}://${host}:${port}/?tola-session=${encodeURIComponent(session)}`);
      socket = opening;
      onSocketChange(opening);

      opening.onopen = () => {
        if (socket !== opening) return;
        reconnectRetries = 0;
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      };
      opening.onmessage = event => {
        if (socket !== opening) return;
        try { onMessage(JSON.parse(event.data)); }
        catch (error) { console.error('[tola] ignored a site update that could not be read:', error); }
      };
      opening.onclose = () => {
        if (socket !== opening) return;
        socket = null;
        onSocketChange(null);
        if (!pageActive()) return;
        attemptReconnect();
      };
      opening.onerror = () => {};
    }

    /// Release the connection before closing it: the page is going away, and a socket whose close
    /// never reports back must not leave the transport owning a connection it cannot reconnect.
    function closeSilently() {
      const relinquished = socket;
      if (!relinquished) return;
      socket = null;
      onSocketChange(null);
      try { relinquished.close(); } catch (_) {}
    }

    /// A changed public generation triggers navigation to renew authentication.
    /// This request must not fetch or parse session tokens.
    async function recoverServerGeneration() {
      if (!generation || socket || reloadRequired()) return;
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 3000);
      try {
        const response = await fetch(location.href, {
          method: 'HEAD', cache: 'no-store', redirect: 'error',
          credentials: 'same-origin', signal: controller.signal,
        });
        const current = response.headers.get('X-Tola-Reload-Generation');
        if ((response.ok || response.status === 404 || response.status === 503)
            && isReloadGeneration(current) && current !== generation
            && !socket && !reloadRequired() && pageActive()) {
          requireReload();
          requestReload();
        }
      } catch (_) {
        // Retry on the next reconnect; the server may still be starting.
      } finally {
        clearTimeout(timeout);
      }
    }

    function attemptReconnect() {
      if (!port || socket || reconnectTimer || !pageActive() || reloadRequired()) return;
      if (document.visibilityState === 'hidden' || navigator.onLine === false) return;
      const delay = reconnectRetries === 0
        ? 500
        : Math.min(1000 * Math.pow(1.3, reconnectRetries - 1), 5000);
      reconnectRetries = Math.min(reconnectRetries + 1, 20);
      reconnectTimer = setTimeout(async () => {
        try {
          if (!pageActive() || document.visibilityState === 'hidden' || navigator.onLine === false) return;
          if (reconnectRetries >= 2) await recoverServerGeneration();
          connect();
        } finally {
          reconnectTimer = null;
        }
      }, delay);
    }

    return {
      configure(nextPort, nextSession, nextGeneration) {
        port = nextPort;
        session = typeof nextSession === 'string' && nextSession ? nextSession : null;
        generation = isReloadGeneration(nextGeneration) ? nextGeneration : null;
      },
      connect,
      closeSilently,
      attemptReconnect,
    };
  }

  function createTolaRuntime() {
    const page = { active: true, ready: false, reloadPending: false, pathPrefix: decodedMount(bootstrap.mount) };
    const facade = {
      revision: null,
      pageAvailability: null,
      activeOutput: null,
      awaitingPublication: false,
      revisionReloadRequired: false,
      revisionUpdates: Promise.resolve(),
      status: { rebuilding: false, errors: 0, warnings: 0 },
      ws: null,
      timings: Object.create(null),
      handleMessage() {},
      requestReload() {},
      saveReloadState() {},
    };

    /// An explicit value is written as the caller measured it; the `performance.mark` entry stays
    /// on the page's own timeline, where a wall-clock measurement does not belong.
    function mark(name, value) {
      if (typeof name !== 'string' || !name) return;
      const now = Number.isFinite(value)
        ? value
        : (typeof performance !== 'undefined' && typeof performance.now === 'function'
          ? performance.now()
          : Date.now());
      facade.timings[name] = now;
      if (typeof performance !== 'undefined' && typeof performance.mark === 'function') {
        try { performance.mark(`tola:${name}`); } catch (_) {}
      }
    }

    const addresses = createSiteAddresses(() => page.pathPrefix);
    const evidence = createResourceEvidence({ addresses });
    const pageState = createPageState({ mark, canRestore: () => !evidence.hasSiteBehavior });
    const devStatus = createDevStatus();
    const replacements = createResourceReplacements({ addresses });
    const patch = createDocumentPatch({ addresses, pageState, replacements });
    const transport = createReloadTransport({
      onMessage: message => facade.handleMessage(message),
      onSocketChange: socket => { facade.ws = socket; },
      requestReload: () => facade.requestReload(),
      requireReload: () => { facade.revisionReloadRequired = true; },
      pageActive: () => page.active,
      reloadRequired: () => facade.revisionReloadRequired,
    });

    function requestReload() {
      if (!page.ready) return;
      mark('reload-requested');
      if (page.active && document.visibilityState === 'visible') {
        facade.saveReloadState();
        location.reload();
      } else {
        page.reloadPending = true;
      }
    }

    async function applyRevision(diff, pageAvailability) {
      if (!diff
        || !addresses.isHexIdentity(diff.from)
        || !addresses.isHexIdentity(diff.to)
        || diff.from === diff.to
        || !Array.isArray(diff.changes)
        || diff.changes.length === 0
        || !addresses.isPageAvailability(pageAvailability)) {
        throw new Error('the site update could not be read');
      }
      if (facade.revision !== diff.from) {
        throw new Error('the page was out of date');
      }
      if (facade.pageAvailability !== pageAvailability) {
        throw new Error('the site now serves different pages');
      }
      // Every dependency is an address compared against the mount, so a mount the page could not
      // decode leaves nothing an in-place update may rely on.
      if (!addresses.mountDecoded) {
        throw new Error('the update could not be applied in place');
      }
      mark('revision-received');
      evidence.drain();
      // Resource Timing omits pending requests, which may still use the old revision.
      if (!evidence.complete) {
        throw new Error('the update could not be applied in place');
      }
      const dependencies = evidence.dependencies();
      if (dependencies.unknown) {
        throw new Error('the update could not be applied in place');
      }
      const outputs = diff.changes.map(change => addresses.validatedOutput(change));
      if (outputs.some(output => !output)) throw new Error('the site update could not be read');
      const callbacks = createUpdateCallbacks(diff, outputs, dependencies, () => page.active);

      let reload = false;
      const stylesheets = new Map();
      const representations = new Map();
      const media = [];
      let documentOutput = null;
      const requestedPath = addresses.outputPath(location.href);
      const behavior = evidence.hasSiteBehavior;
      for (const change of diff.changes) {
        const output = addresses.validatedOutput(change);
        if (!output) {
          reload = true;
          break;
        }
        if (change.operation !== 'removed') representations.set(output.path, output.representation);
        // A new output may replace the fallback serving this address.
        if (change.operation === 'added' && requestedPath !== null
            && output.path === requestedPath) {
          reload = true;
          continue;
        }
        const consumers = dependencies.paths.get(output.path);
        if (output.kind === 'html-document') {
          if (facade.activeOutput === output.path) {
            if (change.operation !== 'modified') reload = true;
            else documentOutput = output;
          } else if (consumers) {
            reload = true;
          }
          continue;
        }
        if (consumers?.dataFetched && !callbacks.accepts(output.path)) {
          reload = true;
          continue;
        }
        if (consumers?.unsupported || consumers?.unsafeFetched) {
          reload = true;
          continue;
        }
        if (change.operation === 'removed') {
          if (consumers && (!callbacks.accepts(output.path) || consumers.stylesheets.length || consumers.cssImages.length || consumers.media.length)) reload = true;
          continue;
        }
        for (const link of consumers?.stylesheets || []) {
          stylesheets.set(link, { path: output.path, representation: output.representation });
        }
        for (const link of consumers?.cssImages || []) {
          if (!stylesheets.has(link)) stylesheets.set(link, { path: addresses.outputPath(link.href), representation: null });
        }
        if (consumers?.media.length) media.push({ output, references: consumers.media });
        if (consumers && !consumers.stylesheets.length && !consumers.cssImages.length && !consumers.media.length
            && !callbacks.accepts(output.path)) reload = true;
      }

      if (reload) {
        throw new Error('the update could not be applied in place');
      }

      const stylesheetUpdates = [];
      const mediaUpdates = [];
      let documentUpdate = null;
      try {
        if (documentOutput) {
          documentUpdate = await (behavior ? patch.prepareClasses : patch.prepare)(documentOutput, diff);
        }
        if (documentUpdate?.classesChanged) {
          for (const link of document.querySelectorAll('link[rel~="stylesheet"][href]:not([data-tola-staging])')) {
            if (!stylesheets.has(link)) stylesheets.set(link, { path: addresses.outputPath(link.href), representation: null });
          }
        }
        // Prepare the document before staging links; settle all resource workers
        // before commit or cleanup.
        const preparations = [
          ...Array.from(stylesheets, ([link, output]) => async () => {
            stylesheetUpdates.push(await replacements.prepareStylesheet(output.path, output.representation, [link], representations));
          }),
          ...media.map(({ output, references }) => async () => {
            mediaUpdates.push(await replacements.prepareMedia(output.path, output.representation, references));
          }),
        ];
        let next = 0;
        let failure = null;
        await Promise.all(Array.from({ length: Math.min(4, preparations.length) }, async () => {
          while (!failure && next < preparations.length) {
            const prepare = preparations[next++];
            try { await prepare(); }
            catch (error) { failure = error; }
          }
        }));
        if (failure) throw failure;
        await callbacks.run();
        if (facade.revision !== diff.from || facade.revisionReloadRequired) {
          throw new Error('the site changed before the update was applied');
        }
        evidence.drain();
        if (!evidence.complete) {
          throw new Error('the update could not be applied in place');
        }
        const currentDependencies = evidence.dependencies();
        if (currentDependencies.unknown) throw new Error('the page changed how it uses site assets');
        const sameTargets = (before = [], after = []) => before.length === after.length
          && before.every((target, index) => target === after[index]);
        for (const output of outputs) {
          if (output.kind === 'html-document') continue;
          const before = dependencies.paths.get(output.path);
          const after = currentDependencies.paths.get(output.path);
          if (after?.unsupported || after?.unsafeFetched || (after?.dataFetched && !callbacks.accepts(output.path))
              || !sameTargets(before?.stylesheets, after?.stylesheets)
              || !sameTargets(before?.cssImages, after?.cssImages)
              || (before?.media.length || 0) !== (after?.media.length || 0)
              || before?.media.some((target, index) => target.element !== after.media[index].element
                || target.attribute !== after.media[index].attribute)) {
            throw new Error('the page changed how it uses site assets');
          }
        }
        if (evidence.hasSiteBehavior && documentUpdate && !documentUpdate.scripted) {
          throw new Error('the page now requires navigation');
        }
        if (documentUpdate) documentUpdate.validate();
        for (const update of stylesheetUpdates) update.validate();
        for (const update of mediaUpdates) update.validate();
        for (const update of stylesheetUpdates) update.commit();
        for (const update of mediaUpdates) update.commit();
        if (documentUpdate) documentUpdate.commit();
      } catch (error) {
        for (const update of stylesheetUpdates) update.dispose();
        for (const update of mediaUpdates) update.dispose();
        throw error;
      }
      facade.revision = diff.to;
      patch.advance(diff.from, diff.to, documentUpdate?.source);
      facade.pageAvailability = pageAvailability;
      mark('revision-committed');
    }

    function handleMessage(message) {
      if (!page.ready) return;
      if (!message || typeof message !== 'object') return;
      if (message.type === 'awaiting' || message.type === 'connected' || message.type === 'revision') {
        facade.revisionUpdates = facade.revisionUpdates
          .then(() => {
            if (facade.revisionReloadRequired) return;
            if (message.type === 'awaiting') {
              // The page already renders a revision the server no longer has.
              if (!facade.awaitingPublication) {
                facade.revisionReloadRequired = true;
                facade.requestReload();
              }
              return;
            }
            if (message.type === 'connected') {
              // Wait for pending resource updates before comparing the document's revision.
              if (!addresses.isHexIdentity(message.revision)
                  || !addresses.isPageAvailability(message.page_availability)
                  || facade.awaitingPublication
                  || (facade.revision && facade.revision !== message.revision)
                  || (facade.pageAvailability && facade.pageAvailability !== message.page_availability)) {
                facade.revisionReloadRequired = true;
                facade.requestReload();
                return;
              }
              facade.revision = message.revision;
              facade.pageAvailability = message.page_availability;
              return;
            }
            return applyRevision(message.diff, message.page_availability);
          })
          .catch(error => {
            facade.revisionReloadRequired = true;
            facade.requestReload();
            console.info('[tola] reloading the page:', error);
          });
        return;
      }
      if (message.type === 'status') {
        if (typeof message.rebuilding !== 'boolean'
            || !isDiagnosticCount(message.errors)
            || !isDiagnosticCount(message.warnings)) {
          console.error('[tola] ignored a status message that could not be read');
          return;
        }
        facade.status = {
          rebuilding: message.rebuilding,
          errors: message.errors,
          warnings: message.warnings,
        };
        devStatus.setStatus(facade.status, facade.awaitingPublication);
      }
    }

    function initialize() {
      if (document.readyState === 'loading' || page.ready) return;
      page.ready = true;
      facade.revision = bootstrap.revision;
      facade.awaitingPublication = bootstrap.revision === null && bootstrap.page_availability === null;
      facade.activeOutput = bootstrap.output;
      facade.pageAvailability = bootstrap.page_availability;
      if (facade.revision && evidence.hasSiteBehavior) patch.initialize(facade.revision);
      mark('page-ready');
      mark('initialized');
      pageState.restoreReloadState();
      transport.configure(bootstrap.port, bootstrap.session, bootstrap.generation);
      transport.connect();
      addEventListener('pagehide', () => {
        page.active = false;
        transport.closeSilently();
      });
      addEventListener('pageshow', () => {
        page.active = true;
        if (page.reloadPending) facade.requestReload();
        else transport.attemptReconnect();
      });
      document.addEventListener('visibilitychange', () => {
        if (document.visibilityState !== 'visible') return;
        if (page.reloadPending) facade.requestReload();
        else transport.attemptReconnect();
      });
      addEventListener('online', () => transport.attemptReconnect());
    }

    facade.handleMessage = handleMessage;
    facade.requestReload = requestReload;
    facade.saveReloadState = () => pageState.saveReloadState();
    evidence.observe();

    return { facade, initialize };
  }

  const runtime = createTolaRuntime();
  window.Tola = runtime.facade;
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', runtime.initialize, { once: true });
  } else {
    runtime.initialize();
  }
})();

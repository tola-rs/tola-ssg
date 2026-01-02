(function() {
  const SOURCE = __TOLA_RECOLOR_SOURCE__;

  function getVar(el, name) {
    const v = getComputedStyle(el).getPropertyValue(name).trim();
    return v || null;
  }

  function getRecolorColor() {
    const root = document.documentElement;
    const body = document.body;

    if (SOURCE === "auto") {
      return getVar(root, '--tola-recolor-value')
          || getComputedStyle(body).color;
    }

    // SOURCE is CSS variable name
    return getVar(root, SOURCE);
  }

  function parseColor(color) {
    // "rgb(255, 255, 255)" → [1, 1, 1]
    const match = color.match(/rgb\((\d+),\s*(\d+),\s*(\d+)\)/);
    if (match) {
      return [match[1]/255, match[2]/255, match[3]/255];
    }
    return [1, 1, 1];  // fallback white
  }

  function updateRecolor() {
    const color = getRecolorColor();
    if (!color) return;

    const [r, g, b] = parseColor(color);
    const filter = document.querySelector('filter#tola-recolor');
    if (!filter) return;

    // Luminance-based switching: black→target, white→black or white
    const targetLum = 0.299*r + 0.587*g + 0.114*b;
    const B = targetLum > 0.5 ? 0 : 1;  // unified B for all channels

    filter.querySelector('feFuncR').setAttribute('tableValues', `${r} ${B}`);
    filter.querySelector('feFuncG').setAttribute('tableValues', `${g} ${B}`);
    filter.querySelector('feFuncB').setAttribute('tableValues', `${b} ${B}`);
  }

  // Initialize
  updateRecolor();

  // Listen for theme changes
  window.matchMedia('(prefers-color-scheme: dark)')
    .addEventListener('change', updateRecolor);

  // Expose for manual theme switching
  window.TolaRecolor = { update: updateRecolor };
})();

# UI Prototype

Generate several radically different UI variations on one route, switchable via a floating bottom bar. User flips variants in the browser, picks one (or steals bits from each), discards the rest.

Logic/state question rather than looks → wrong branch: use [LOGIC.md](LOGIC.md).

## When this shape fits

- "What should this page look like?"
- "I want to see a few options for this dashboard before committing."
- "Try a different layout for the settings screen."
- User otherwise spends a day picking between three vague mockups in their head.

## Two sub-shapes — strongly prefer A

Variants much easier to judge butting against the rest of the app: real header, sidebar, data, density. Throwaway route alone = vacuum — every variant looks fine isolated. Default A whenever a plausible existing page can host variants; reach for B only if genuinely no nearby home.

### A: adjust an existing page (preferred)

Route already exists. Variants render on the same route, gated by `?variant=` URL search param. Existing data fetching, params, auth stay; only rendering swaps. Default — pick unless a specific reason not to.

Prototype lacks a page but would naturally live inside one (new dashboard section, new settings card, new step in an existing flow) → still A. Mount variants inside the host page.

### B: a new page (last resort)

Only when the thing genuinely has no existing page to live inside (entirely new top-level surface, or a flow not embeddable anywhere sensible).

Create a throwaway route following project routing convention. Don't invent a new top-level structure. Name it obviously prototype (word `prototype` in path or filename). Same `?variant=` pattern.

Before committing to B, sanity-check: really no existing page to embed in? Empty route hides design problems a populated one exposes.

Floating bottom bar identical in both sub-shapes.

## Process

### 1. State question, pick N

Default 3 variants. >5 stops being radically different, starts being noise — cap there.

Plan in one line, in the prototype's location or a top-of-file comment:

> "Three variants of the settings page, switchable via `?variant=`, on the existing `/settings` route."

Works whether the user is here to push back or not.

### 2. Generate radically different variants

Draft each. Hold each to:

- Page purpose + data it can access.
- Project component library / styling system (TailwindCSS, shadcn, MUI, plain CSS, whatever).
- Clear exported component name, e.g. `VariantA`, `VariantB`, `VariantC`.

Variants MUST be structurally different — layout, information hierarchy, primary affordance — not just colours. Three slightly-tweaked card grids isn't a UI prototype, it's wallpaper. Two drafts too similar → redo one with explicit "do not use a card grid" guidance.

### 3. Wire together

Single switcher component on the route:

```tsx
// pseudo-code, adapt to the project's framework
const variant = searchParams.get('variant') ?? 'A';
return (
  <>
    {variant === 'A' && <VariantA {...data} />}
    {variant === 'B' && <VariantB {...data} />}
    {variant === 'C' && <VariantC {...data} />}
    <PrototypeSwitcher variants={['A','B','C']} current={variant} />
  </>
);
```

A: keep all existing data fetching above the switcher; only the rendered subtree changes per variant.

B: throwaway route under `/prototype/<name>` mounts the same switcher.

### 4. Build the floating switcher

Small fixed-position bar bottom-centre, three pieces:

- **Left arrow**: cycles prev variant (wraps).
- **Variant label**: current variant key + exported name if any. e.g. `B (Sidebar layout)`.
- **Right arrow**: cycles forward (wraps).

Behaviour:

- Arrow click updates the URL search param (framework router — `router.replace` on Next, `navigate` on React Router, etc) → variant shareable, reload-stable.
- `←`/`→` keys also cycle. Don't intercept arrow keys when `<input>`, `<textarea>`, or `[contenteditable]` is focused.
- Visually distinct from the page (e.g. high-contrast pill, subtle shadow) → obviously not part of the design evaluated.
- Hidden in production builds: gate on `process.env.NODE_ENV !== 'production'` or equivalent → a stray prototype merge can't ship the bar to users.

Single shared component so both sub-shapes reuse it. Locate it wherever shared UI lives in the project.

### 5. Hand over

Surface the URL (and the `?variant=` keys). User flips through whenever they get to it. Interesting feedback is usually "I want the header from B with the sidebar from C" — the actual design wanted.

### 6. Capture the answer, clean up

Variant won → capture the answer (which and why), then capture the prototype as [SKILL](SKILL.md) describes. Fold the winner into real code; move the rest to the throwaway branch, not main:

- A: fold winner into the existing page; drop losing variants + switcher from main.
- B: promote the winning variant to a real route; drop the throwaway route + switcher from main.

Full variant set = primary source → throwaway branch, not bin: variants + switcher left in main rot fast, confuse the next reader.

## Anti-patterns

- **Variants differ only in colour or copy.** A tweak, not a prototype. Real variants disagree about structure.
- **Sharing too much code between variants.** A shared `<Header>` is fine; a shared `<Layout>` defeats the point. Each variant is free to throw out the layout.
- **Wiring variants to real mutations.** Read-only is fine. Needs to mutate → point at a stub: the question is "what should this look like", not "does the backend work".
- **Promoting the prototype directly to production.** Variant code was written under prototype constraints (no tests, minimal error handling). Rewrite it properly when folding in.

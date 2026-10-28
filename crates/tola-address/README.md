# tola-address

The address types used across Tola: slugs, routes, output paths, and browser URLs. They are
distinct values, and this crate owns the conversions between them.

- **route** (`UrlPath`): a decoded site-root address such as `/posts/hello/`. A trailing slash
  is part of the address: `/x/` is a directory, `/x` is a file. Percent decoding happens once,
  when an encoded URL path becomes a route.
- **output path** (`OutputPath`): where a document is written. `/x/` becomes `x/index.html`;
  `/x` stays `x`. `route_for_output` is the reverse (`index.html` -> `/`). One output path
  has one owner.
- **browser URL**: `browser_url` applies a deployment mount (`SiteUrlMount`) and an optional
  origin to a route; `browser_location` renders a redirect URI.
- **slug**: `slugify_segments` slugs ordered segments and validates the joined route, so a
  segment cannot change the hierarchy. Turning text into a slug belongs to `tola-slugify`.

```rust
use tola_address::{
    OutputPath, SiteUrlMount, UrlPath, browser_url, route_for_output, slugify_segments,
};
use tola_slugify::NamingRules;

// segments -> route, each segment slugged through tola-slugify
let route = slugify_segments(
    &["posts".to_owned(), "Héllo World".to_owned()],
    NamingRules::default(),
)?;
assert_eq!(route.as_str(), "/posts/héllo-world/");

// route -> output path, and back to the canonical route
let output = OutputPath::from_route(&route);
assert_eq!(output.as_str(), "posts/héllo-world/index.html");
assert_eq!(route_for_output(&output).as_str(), "/posts/héllo-world/");

// route + deployment mount -> browser URL
let mount = SiteUrlMount::from_base_path("/blog/")?;
assert_eq!(browser_url(&route, &mount, None), "/blog/posts/h%C3%A9llo-world/");

// an encoded URL path is decoded once, at this boundary
let route = UrlPath::parse("/posts/caf%C3%A9/")?;
assert_eq!(route.as_str(), "/posts/café/");
assert_eq!(route.to_encoded(), "/posts/caf%C3%A9/");
```

`SiteReference` keeps a decoded route with its query and serialized fragment; `decoded_fragment()`
gives a decoded view and `to_browser_url()` renders the mounted route with its suffixes. Fragment
escapes stay intact until a consumer interprets them: HTML matches an id against the serialized
spelling before trying the decoded one. `resolve_browser_reference` resolves a document base and
classifies the destination; `split_destination` splits the raw string without decoding.

`portable_collision_key` folds one component (case and Unicode normalization);
`portable_keys_overlap` decides whether two paths claim the same file or an ancestor of each
other; `portable_key_is_reserved` marks the reserved `_tola` namespace.

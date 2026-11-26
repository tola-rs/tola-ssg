# tola-address

Site routes (`UrlPath`), output paths (`OutputPath`), browser URLs (`SiteUrlMount`), and link references (`SiteRef`).

- `UrlPath`: decoded site-root route (`/posts/hello/`; trailing slash marks a directory).
- `OutputPath`: file path under the publish root (`/x/` -> `x/index.html`, `/x` -> `x`).
- `browser_url` / `browser_location`: apply a `base-path` mount and optional origin.
- `slugify_segments`: slug each segment via `tola-slugify` and validate the joined route.

```rust
use tola_address::{
    OutputPath, SiteUrlMount, UrlPath, browser_url, route_for_output, slugify_segments,
};
use tola_slugify::NamingRules;

let route = slugify_segments(
    &["posts".to_owned(), "Héllo World".to_owned()],
    NamingRules::default(),
)?;
assert_eq!(route.as_str(), "/posts/héllo-world/");

let output = OutputPath::from_route(&route);
assert_eq!(output.as_str(), "posts/héllo-world/index.html");
assert_eq!(route_for_output(&output).as_str(), "/posts/héllo-world/");

let mount = SiteUrlMount::from_base_path("/blog/")?;
assert_eq!(browser_url(&route, &mount, None), "/blog/posts/h%C3%A9llo-world/");
```

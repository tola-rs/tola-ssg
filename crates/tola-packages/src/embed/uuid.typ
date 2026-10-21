// @tola/uuid:0.0.0 - deterministic UUID operations

/// Convert a UUID string to lowercase, hyphenated form.
///
/// Accepts 32 hexadecimal digits, a hyphenated UUID, or a hyphenated UUID wrapped in braces
/// or prefixed with `urn:uuid:`. Invalid values are errors; whitespace is not trimmed.
#import "@tola/host:0.0.0": uuid-normalize as normalize

/// Compute the deterministic version 5 UUID for a namespace and name.
///
/// The namespace is a UUID string. A string name is encoded as UTF-8 without Unicode
/// normalization; a bytes name is used as-is. The result is lowercase and hyphenated.
/// Changing either input changes the identity. Persist an ID when its name or path may change.
///
/// ```typst
/// #import "@tola/uuid:0.0.0": namespace, v5
/// #assert.eq(v5(namespace.dns, "www.example.com"), "2ed6657d-e927-568b-95e1-2665a8aea6a2")
/// ```
#import "@tola/host:0.0.0": uuid-v5 as v5

/// Standard UUID namespaces: `dns`, `url`, `oid`, and `x500`.
#let namespace = (
  dns: "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
  url: "6ba7b811-9dad-11d1-80b4-00c04fd430c8",
  oid: "6ba7b812-9dad-11d1-80b4-00c04fd430c8",
  x500: "6ba7b814-9dad-11d1-80b4-00c04fd430c8",
)

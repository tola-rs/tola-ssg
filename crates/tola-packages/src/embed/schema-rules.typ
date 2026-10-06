// The caller passes its own `check` and `issue`, so these rules reuse the core's absence and
// provenance handling.
#let rules(check, issue) = {
  let length-of(value) = {
    if type(value) == str { return value.clusters().len() }
    if type(value) == array or type(value) == dictionary { return value.len() }
    none
  }

  let min-length(member, minimum) = {
    assert(
      type(minimum) == int and minimum >= 0,
      message: "min-length: `minimum` must be a nonnegative integer",
    )
    check(member, value => {
      let value-length = length-of(value)
      if value-length == none {
        return (issue("expected a string, array, or dictionary", code: "schema.type"),)
      }
      if value-length >= minimum { return () }
      (issue("must have length at least `" + repr(minimum) + "`", code: "schema.min-length"),)
    })
  }

  let max-length(member, maximum) = {
    assert(
      type(maximum) == int and maximum >= 0,
      message: "max-length: `maximum` must be a nonnegative integer",
    )
    check(member, value => {
      let value-length = length-of(value)
      if value-length == none {
        return (issue("expected a string, array, or dictionary", code: "schema.type"),)
      }
      if value-length <= maximum { return () }
      (issue("must have length at most `" + repr(maximum) + "`", code: "schema.max-length"),)
    })
  }

  let matches(member, expression) = {
    assert(
      type(expression) == regex,
      message: "matches: `expression` must be a regular expression",
    )
    check(member, value => {
      if type(value) != str {
        return (issue("expected a string", code: "schema.type"),)
      }
      if value.contains(expression) { return () }
      (issue("must match the declared regular expression", code: "schema.matches"),)
    })
  }

  let is-number(value) = type(value) == int or type(value) == float
  let is-nan(value) = type(value) == float and value.is-nan()

  let at-least(member, minimum) = {
    assert(
      is-number(minimum) and not is-nan(minimum),
      message: "at-least: `minimum` must be an integer or float other than NaN",
    )
    check(member, value => {
      if not is-number(value) {
        return (issue("expected an integer or float", code: "schema.type"),)
      }
      if not is-nan(value) and value >= minimum { return () }
      (issue("must be at least `" + repr(minimum) + "`", code: "schema.at-least"),)
    })
  }

  let at-most(member, maximum) = {
    assert(
      is-number(maximum) and not is-nan(maximum),
      message: "at-most: `maximum` must be an integer or float other than NaN",
    )
    check(member, value => {
      if not is-number(value) {
        return (issue("expected an integer or float", code: "schema.type"),)
      }
      if not is-nan(value) and value <= maximum { return () }
      (issue("must be at most `" + repr(maximum) + "`", code: "schema.at-most"),)
    })
  }

  // RFC 5322 atext, without quoted strings, comments, or folding whitespace.
  let email-atom = regex("\\A[A-Za-z0-9!#$%&'*+/=?^_`{|}~-]+\\z")
  let domain-label-pattern = regex("\\A[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\\z")

  let is-email(address) = {
    // SMTP's 256-byte path includes the surrounding angle brackets (RFC 5321 §4.5.3.1).
    if address.len() > 254 { return false }
    let mailbox = address.split("@")
    if mailbox.len() != 2 { return false }
    let (local-part, domain) = mailbox
    if local-part.len() > 64 { return false }
    (
      local-part.split(".").all(atom => atom.contains(email-atom)) and
      domain.split(".").all(domain-label => domain-label.contains(domain-label-pattern))
    )
  }

  // RFC 3986 dec-octet grammar excludes ambiguous leading-zero spellings without conversion.
  let decimal-octet = "(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])"
  let ipv4-pattern = regex("\\A" + decimal-octet + "(?:\\." + decimal-octet + "){3}\\z")
  let is-ipv4(address) = address.contains(ipv4-pattern)
  let hex-group = regex("\\A[0-9A-Fa-f]{1,4}\\z")

  let is-ipv6(address) = {
    let compression-sides = address.split("::")
    if compression-sides.len() > 2 { return false }
    let compressed = compression-sides.len() == 2
    let group-count = 0
    for (side-index, side) in compression-sides.enumerate() {
      if side == "" { continue }
      let groups = side.split(":")
      for (group-index, group) in groups.enumerate() {
        if group.contains(".") {
          // An IPv4 suffix occupies the final two groups, never the side before `::`.
          if side-index != compression-sides.len() - 1 or group-index != groups.len() - 1 {
            return false
          }
          if not is-ipv4(group) { return false }
          group-count += 2
        } else {
          if not group.contains(hex-group) { return false }
          group-count += 1
        }
      }
    }
    // RFC 4291 §2.2 permits `::` only when it replaces at least one 16-bit group.
    if compressed { group-count < 8 } else { group-count == 8 }
  }

  // RFC 3987 ucschar excludes private-use and noncharacter ranges; only queries add iprivate.
  let iri-unreserved = (
    "A-Za-z0-9._~\\-"
    + "\\x{A0}-\\x{D7FF}\\x{F900}-\\x{FDCF}\\x{FDF0}-\\x{FFEF}"
    + "\\x{10000}-\\x{1FFFD}\\x{20000}-\\x{2FFFD}\\x{30000}-\\x{3FFFD}"
    + "\\x{40000}-\\x{4FFFD}\\x{50000}-\\x{5FFFD}\\x{60000}-\\x{6FFFD}"
    + "\\x{70000}-\\x{7FFFD}\\x{80000}-\\x{8FFFD}\\x{90000}-\\x{9FFFD}"
    + "\\x{A0000}-\\x{AFFFD}\\x{B0000}-\\x{BFFFD}\\x{C0000}-\\x{CFFFD}"
    + "\\x{D0000}-\\x{DFFFD}\\x{E1000}-\\x{EFFFD}"
  )
  let iri-private = "\\x{E000}-\\x{F8FF}\\x{F0000}-\\x{FFFFD}\\x{100000}-\\x{10FFFD}"
  let iri-component-pattern(extra-characters) = regex(
    "\\A(?:[" + iri-unreserved + "!$&'()*+,;=" + extra-characters + "]|%[0-9A-Fa-f]{2})*\\z",
  )
  let registered-name-pattern = iri-component-pattern("")
  let userinfo-pattern = iri-component-pattern(":")
  let url-path-pattern = iri-component-pattern(":@/")
  let url-query-pattern = iri-component-pattern(":@/?" + iri-private)
  let url-fragment-pattern = iri-component-pattern(":@/?")
  let ipvfuture-pattern = regex("\\A[vV][0-9A-Fa-f]+\\.[A-Za-z0-9._~!$&'()*+,;=:\\-]+\\z")
  let url-whitespace-or-control = regex("[\\s\\p{Cc}]")
  let http-url-pattern = regex(
    "\\A([hH][tT][tT][pP][sS]?)://([^/?#]*)(/[^?#]*)?(?:\\?([^#]*))?(?:#(.*))?\\z",
  )
  // An empty port uses the scheme default (RFC 9110); decimal spelling never needs conversion.
  let port-pattern = regex(
    "\\A0*(?:[0-9]{1,4}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5])?\\z",
  )

  let is-http-authority(authority) = {
    let userinfo-parts = authority.split("@")
    if userinfo-parts.len() > 2 { return false }
    if userinfo-parts.len() == 2 and not userinfo-parts.first().contains(userinfo-pattern) {
      return false
    }
    let host-port = userinfo-parts.last()
    if host-port == "" { return false }
    if host-port.starts-with("[") {
      let closing-bracket = host-port.position("]")
      if closing-bracket == none { return false }
      let address = host-port.slice(1, closing-bracket)
      if not is-ipv6(address) and not address.contains(ipvfuture-pattern) { return false }
      let port-suffix = host-port.slice(closing-bracket + 1)
      if port-suffix == "" { return true }
      return port-suffix.starts-with(":") and port-suffix.slice(1).contains(port-pattern)
    }
    let host-parts = host-port.split(":")
    if host-parts.len() > 2 { return false }
    let host = host-parts.first()
    if host == "" or not host.contains(registered-name-pattern) { return false }
    host-parts.len() == 1 or host-parts.last().contains(port-pattern)
  }

  let is-http-url(address, https-only: false) = {
    if address.contains(url-whitespace-or-control) { return false }
    let url-match = address.match(http-url-pattern)
    if url-match == none { return false }
    let (scheme, authority, url-path, url-query, url-fragment) = url-match.captures
    if https-only and lower(scheme) != "https" { return false }
    if not is-http-authority(authority) { return false }
    if url-path != none and not url-path.contains(url-path-pattern) { return false }
    if url-query != none and not url-query.contains(url-query-pattern) { return false }
    if url-fragment != none and not url-fragment.contains(url-fragment-pattern) { return false }
    true
  }

  (
    min-length: min-length,
    max-length: max-length,
    matches: matches,
    at-least: at-least,
    at-most: at-most,
    email: check(str, value => {
      if is-email(value) { return () }
      (issue("expected an unquoted ASCII email address", code: "schema.email"),)
    }),
    ipv4: check(str, value => {
      if is-ipv4(value) { return () }
      (issue("expected an IPv4 address", code: "schema.ipv4"),)
    }),
    ipv6: check(str, value => {
      if is-ipv6(value) { return () }
      (issue("expected an IPv6 address", code: "schema.ipv6"),)
    }),
    ip: check(str, value => {
      if is-ipv4(value) or is-ipv6(value) { return () }
      (issue("expected an IPv4 or IPv6 address", code: "schema.ip"),)
    }),
    http-url: check(str, value => {
      if is-http-url(value) { return () }
      (issue("expected an absolute HTTP(S) URL with valid syntax", code: "schema.http-url"),)
    }),
    https-url: check(str, value => {
      if is-http-url(value, https-only: true) { return () }
      (issue("expected an absolute HTTPS URL with valid syntax", code: "schema.https-url"),)
    }),
  )
}

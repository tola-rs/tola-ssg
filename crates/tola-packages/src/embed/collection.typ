// @tola/collection:0.0.0 - pick, group, index, and navigate arrays of your own values
//
// Members are your own values: dictionaries, strings, or anything else you can put in an array.
// Each operation takes the accessor functions you supply, so field names, order, and hierarchy
// conventions are yours to choose. `key` identifies a member, `keys` gives the memberships a
// member carries, and `parent` derives a member's container key.
//
// `key` and `keys` receive a member; `parent` receives a key, including keys that have no member
// in the array. A hierarchy walk can revisit shared ancestors, so `parent` must return the same
// container key for a given key, and must reach `none` after finitely many steps.

/// Return the entries of `fields` named by the string keys in `keys`, in the order you list them.
///
/// Every key in `keys` must be a string. Only keys that exist in `fields` are copied, and a key
/// listed twice counts once. Values are copied unchanged, `none` included. A dot in a key is an
/// ordinary character, so `"a.b"` names one key.
///
/// Example - copy selected fields:
///
/// ```typst
/// #import "@tola/collection:0.0.0": pick
/// #let fields = (title: "Intro", "a.b": 1, draft: true)
/// #assert.eq(pick(fields, ("a.b", "title", "a.b", "missing")), ("a.b": 1, title: "Intro"))
/// ```
///
/// - fields (dictionary): the dictionary to copy from.
/// - keys (array): the string keys to copy.
/// -> dictionary
#let pick(fields, keys) = {
  assert(type(fields) == dictionary, message: "pick expects a dictionary")
  assert(type(keys) == array, message: "pick expects an array of string keys")
  let selected = (:)
  for field in keys {
    assert(type(field) == str, message: "pick expects string keys")
    if field in fields and field not in selected {
      selected.insert(field, fields.at(field))
    }
  }
  selected
}

/// Return a dictionary that maps one string key to one member, in input order.
///
/// A duplicate key is an error.
///
/// Example - index members by one key each:
///
/// ```typst
/// #import "@tola/collection:0.0.0": index-by
/// #let pages = ((path: "index.html", title: "Home"), (path: "404.html", title: "Not Found"))
/// #let by-path = index-by(pages, key: page => page.path)
/// #assert.eq(by-path.at("404.html"), pages.at(1))
/// ```
///
/// Related: group-by
/// - members (array): the members to index.
/// - key (function): called once per member; must return a string, never an array or `none`.
/// -> dictionary
#let index-by(members, key: value => value) = {
  import "keys.typ": duplicate-key-message

  assert(type(members) == array, message: "index-by expects an array")
  let by-key = (:)
  for member in members {
    let member-key = key(member)
    assert(type(member-key) == str, message: "index-by `key` must return a string")
    if member-key in by-key {
      panic(duplicate-key-message("index-by", member-key))
    }
    by-key.insert(member-key, member)
  }
  by-key
}

/// Group the members by one string key each.
///
/// Group order follows each key's first appearance, and members keep input order. For members
/// with several memberships, use `group-by-keys`.
///
/// Example - group members by one key each:
///
/// ```typst
/// #import "@tola/collection:0.0.0": group-by
/// #let pages = ((section: "Guides", title: "Setup"), (section: "Blog", title: "Hello"), (section: "Guides", title: "Deploy"))
/// #let groups = group-by(pages, key: page => page.section)
/// #assert.eq(groups.keys(), ("Guides", "Blog"))
/// #assert.eq(groups.at("Guides").map(page => page.title), ("Setup", "Deploy"))
/// ```
///
/// Related: group-by-keys
/// - members (array): the members to group.
/// - key (function): called once per member; must return a string.
/// -> dictionary
#let group-by(members, key: value => value) = {
  assert(type(members) == array, message: "group-by expects an array")
  let groups = (:)
  for member in members {
    let group-key = key(member)
    assert(type(group-key) == str, message: "group-by `key` must return a string")
    if group-key not in groups { groups.insert(group-key, ()) }
    groups.at(group-key).push(member)
  }
  groups
}

/// Group members by the string keys each member carries.
/// Group order follows each key's first appearance, and members keep input order. A member with an
/// empty `keys` array joins no group, and a repeated key joins once.
///
/// Example - group members by their tag lists:
///
/// ```typst
/// #import "@tola/collection:0.0.0": group-by-keys
/// #let notes = ((title: "Build", tags: ("web", "typst")), (title: "Read", tags: ("typst",)))
/// #let by-tag = group-by-keys(notes, keys: note => note.tags)
/// #assert.eq(by-tag.at("typst"), notes)
/// ```
///
/// Example - count a repeated key once and skip an empty key list:
///
/// ```typst
/// #import "@tola/collection:0.0.0": group-by-keys
/// #let notes = ((title: "A", tags: ()), (title: "B", tags: ("web", "web")), (title: "C", tags: ("web",)))
/// #let by-tag = group-by-keys(notes, keys: note => note.tags)
/// #assert.eq(by-tag.keys(), ("web",))
/// #assert.eq(by-tag.at("web").map(note => note.title), ("B", "C"))
/// ```
///
/// Related: group-by
/// - members (array): the members to group.
/// - keys (function): called once per member; returns the member's string keys.
/// -> dictionary
#let group-by-keys(members, keys: value => value) = {
  import "keys.typ": memberships-of

  assert(type(members) == array, message: "group-by-keys expects an array")
  let groups = (:)
  for member in members {
    for group-key in memberships-of(member, keys, "group-by-keys").dedup() {
      assert(type(group-key) == str, message: "group-by-keys `keys` must return string keys")
      if group-key not in groups { groups.insert(group-key, ()) }
      groups.at(group-key).push(member)
    }
  }
  groups
}

/// Keep the members whose `keys` array contains any one of the `wanted` values.
/// With `match: "all"`, keep only the members that have every wanted value.
///
/// Both arrays hold ordinary values: `(none,)` counts as one membership, `()` as none. Input
/// order is kept. When `wanted` is empty, `"any"` selects no members and `"all"` selects every
/// member.
///
/// Example - filter members by their memberships:
///
/// ```typst
/// #import "@tola/collection:0.0.0": select-members
/// #let notes = ((title: "A", tags: ("web", "typst")), (title: "B", tags: ("web",)), (title: "C", tags: ()))
/// #let tagged = select-members(notes, ("web",), keys: note => note.tags)
/// #assert.eq(tagged.map(note => note.title), ("A", "B"))
/// #assert.eq(select-members(notes, ("web", "typst"), keys: note => note.tags, match: "all").map(note => note.title), ("A",))
/// ```
///
/// Example - select with an empty `wanted` array:
///
/// ```typst
/// #import "@tola/collection:0.0.0": select-members
/// #let notes = ((title: "A", tags: ("web",)), (title: "B", tags: ("typst",)))
/// #assert.eq(select-members(notes, (), keys: note => note.tags), ())
/// #assert.eq(select-members(notes, (), keys: note => note.tags, match: "all"), notes)
/// ```
///
/// Related: group-by-keys
/// - members (array): the members to filter.
/// - wanted (array): the memberships a member must have.
/// - keys (function): returns the memberships one member has.
/// - match ("any" | "all"): whether one wanted membership is enough, or all of them are
///   required.
/// -> array
#let select-members(members, wanted, keys: value => value, match: "any") = {
  import "keys.typ": memberships-of

  assert(type(members) == array, message: "select-members expects an array")
  assert(type(wanted) == array, message: "select-members `wanted` must be an array")
  assert(match in ("any", "all"), message: "select-members `match` must be \"any\" or \"all\"")
  members.filter(member => {
    let available = memberships-of(member, keys, "select-members")
    if match == "all" {
      wanted.all(value => value in available)
    } else {
      wanted.any(value => value in available)
    }
  })
}

/// Return the neighbors of the member keyed `at`, as a dictionary.
///
/// If no member has that key, the result is `none`.
///
/// A `before` or `after` key appears only when that neighbor exists, and a neighbor may itself
/// be `none`. Member keys must be unique, even when `at` is absent. Keys may be any comparable
/// value, including `none`; `index-by` is stricter and requires strings.
///
/// Example - find a member's neighbors:
///
/// ```typst
/// #import "@tola/collection:0.0.0": adjacent
/// #let pages = ((id: "a", title: "A"), (id: "b", title: "B"), (id: "c", title: "C"))
/// #let neighbors = adjacent(pages, "b", key: page => page.id)
/// #assert.eq(neighbors.before.title, "A")
/// #assert.eq(neighbors.after.title, "C")
/// #assert.eq(adjacent(pages, "z", key: page => page.id), none)
/// ```
///
/// Example - walk the ends of a chain:
///
/// ```typst
/// #import "@tola/collection:0.0.0": adjacent
/// #let steps = (none, "middle", "last")
/// #assert.eq(adjacent(steps, none), (after: "middle"))
/// #assert.eq(adjacent(steps, "last"), (before: "middle"))
/// #assert.eq(adjacent(steps, "middle"), (before: none, after: "last"))
/// ```
///
/// Example - link to the previous and next page of a section:
///
/// ```typst site
/// #import "@tola/address:0.0.0": route, route-to-output
/// #import "@tola/collection:0.0.0": adjacent
/// #import "@tola/source:0.0.0": all-sources
///
/// // The guide's pages, in the order the section declares.
/// #let pages = (
///   all-sources()
///     .filter(source => source.path.starts-with("guide/"))
///     .sorted(key: source => (source.meta.order, source.id))
///     .map(source => (
///       output: route-to-output(route(source.route-segments)),
///       title: source.meta.title,
///     ))
/// )
///
/// #let neighbors = adjacent(pages, "guide/install/index.html", key: page => page.output)
/// #assert.eq(neighbors.before.title, "Guide")
/// #assert.eq(neighbors.after.title, "Deploy")
/// ```
///
/// Related: window, index-by
/// - members (array): the members to search.
/// - at (any): the key whose neighbors are returned.
/// - key (function): returns one member's key.
/// -> none | dictionary
#let adjacent(members, at, key: value => value) = {
  import "keys.typ": position-of

  let position = position-of(members, at, key, "adjacent")
  if position == none { return none }
  let neighbors = (:)
  if position > 0 { neighbors.insert("before", members.at(position - 1)) }
  if position + 1 < members.len() { neighbors.insert("after", members.at(position + 1)) }
  neighbors
}

/// Return up to `before` preceding and up to `after` following members, excluding the anchor.
/// The result keeps input order. If no member has the key `at`, the result is `none`.
///
/// Member keys must be unique, even when `at` is absent.
///
/// Example - collect a window around a member:
///
/// ```typst
/// #import "@tola/collection:0.0.0": window
/// #let pages = ((id: "a"), (id: "b"), (id: "c"), (id: "d"))
/// #let around = window(pages, "c", key: page => page.id, before: 1, after: 1)
/// #assert.eq(around.map(page => page.id), ("b", "d"))
/// #assert.eq(window(pages, "z", key: page => page.id), none)
/// ```
///
/// Example - keep one side of the anchor:
///
/// ```typst
/// #import "@tola/collection:0.0.0": window
/// #let pages = ((id: "a"), (id: "b"), (id: "c"))
/// #assert.eq(window(pages, "b", key: page => page.id), ())
/// #assert.eq(window(pages, "b", key: page => page.id, before: 1), (pages.at(0),))
/// #assert.eq(window(pages, "c", key: page => page.id, before: 5), (pages.at(0), pages.at(1)))
/// ```
///
/// Related: adjacent
/// - members (array): the members to search.
/// - at (any): the key the window is centered on.
/// - key (function): returns one member's key.
/// - before (int): how many preceding members to include; `0` includes none. Must be a
///   nonnegative integer.
/// - after (int): how many following members to include; `0` includes none. Must be a
///   nonnegative integer.
/// -> none | array
#let window(members, at, key: value => value, before: 0, after: 0) = {
  import "keys.typ": position-of

  assert(
    type(before) == int and before >= 0,
    message: "window `before` must be a nonnegative integer",
  )
  assert(
    type(after) == int and after >= 0,
    message: "window `after` must be a nonnegative integer",
  )
  let position = position-of(members, at, key, "window")
  if position == none { return none }
  let leading = members.slice(calc.max(0, position - before), position)
  let trailing = members.slice(position + 1, count: calc.min(after, members.len() - position - 1))
  leading + trailing
}

/// Return the direct children of a container key, in input order; `of` must not be `none`.
///
/// The container may be absent from `members`. `parent` returns a member key's container key, or
/// `none` at the root. Member keys must be unique and non-`none`. Only direct edges are examined,
/// and a self-edge is an error.
///
/// Example - list a container's direct children:
///
/// ```typst
/// #import "@tola/collection:0.0.0": children
/// #let pages = ((id: "root"), (id: "api"), (id: "usage"), (id: "faq"))
/// #let parents = (api: "root", usage: "api", faq: "root", root: none)
/// #let kids = children(pages, "root", key => parents.at(key), key: page => page.id)
/// #assert.eq(kids.map(page => page.id), ("api", "faq"))
/// ```
///
/// Example - list children of a container absent from members:
///
/// ```typst
/// #import "@tola/collection:0.0.0": children
/// #let pages = ((id: "intro"), (id: "api"))
/// #let parents = (intro: "guide", api: "guide")
/// #assert.eq(children(pages, "guide", key => parents.at(key), key: page => page.id).map(page => page.id), ("intro", "api"))
/// ```
///
/// Related: descendants
/// - members (array): the members to search.
/// - of (any): the container key whose direct children are returned.
/// - parent (function): receives a member key rather than the member; returns its container key,
///   or `none` at the root.
/// - key (function): called once per member; returns the member's key.
/// -> array
#let children(members, of, parent, key: value => value) = {
  import "keys.typ": hierarchy-keys, parent-key

  assert(of != none, message: "children `of` cannot be `none`")
  let member-keys = hierarchy-keys(members, key, "children")
  let children = ()
  for (position, member-key) in member-keys.enumerate() {
    if parent-key(member-key, parent, "children") == of {
      children.push(members.at(position))
    }
  }
  children
}

/// Return the descendants of a container key, in input order, excluding the container.
/// `of` must not be `none`, and the container may be absent from `members`.
///
/// The result keeps input order. Every member's full parent chain is checked for cycles, including
/// beyond a matching container; chains continue through keys with no member in `members`. Member
/// keys must be unique and non-`none`, and `parent` returns `none` at the root.
///
/// Every member's chain is checked even when the container is absent. When parent lookups are
/// expensive, reuse a precomputed key-to-parent dictionary.
///
/// Example - list every descendant of a container:
///
/// ```typst
/// #import "@tola/collection:0.0.0": descendants
/// #let pages = ((id: "root"), (id: "api"), (id: "usage"), (id: "faq"))
/// #let parents = (api: "root", usage: "api", faq: "root", root: none)
/// #let nested = descendants(pages, "root", key => parents.at(key), key: page => page.id)
/// #assert.eq(nested.map(page => page.id), ("api", "usage", "faq"))
/// ```
///
/// Example - follow a chain through a key that is not a member:
///
/// ```typst
/// #import "@tola/collection:0.0.0": descendants
/// #let pages = ((id: "intro"), (id: "api"))
/// #let parents = (intro: "guide", api: "intro", guide: none)
/// #assert.eq(descendants(pages, "guide", key => parents.at(key), key: page => page.id).map(page => page.id), ("intro", "api"))
/// ```
///
/// Related: children
/// - members (array): the members to search.
/// - of (any): the container key whose descendants are returned.
/// - parent (function): returns a member key's container key, or `none` at the root.
/// - key (function): returns one member's key.
/// -> array
#let descendants(members, of, parent, key: value => value) = {
  import "keys.typ": hierarchy-keys, parent-chain

  assert(of != none, message: "descendants `of` cannot be `none`")
  let member-keys = hierarchy-keys(members, key, "descendants")
  let descendants = ()
  for (position, member-key) in member-keys.enumerate() {
    if of in parent-chain(member-key, parent, "descendants") {
      descendants.push(members.at(position))
    }
  }
  descendants
}

/// Return the ancestors of the member keyed `at`, from the root down to its parent.
///
/// Only ancestors present in `members` are included; the member itself is excluded. Member keys
/// must be unique, and neither they nor `at` may be `none`; an anchor absent from `members`
/// returns `none`. The walk ends when `parent` returns `none`; chains continue through keys with
/// no member in `members`, and a repeated key in the traversed chain is an error.
///
/// Example - walk a member's ancestor chain:
///
/// ```typst
/// #import "@tola/collection:0.0.0": ancestors
/// #let pages = ((id: "root"), (id: "api"), (id: "usage"), (id: "faq"))
/// #let parents = (api: "root", usage: "api", root: none)
/// #let trail = ancestors(pages, "usage", key => parents.at(key), key: page => page.id)
/// #assert.eq(trail.map(page => page.id), ("root", "api"))
/// ```
///
/// Related: lineage
/// - members (array): the members to search.
/// - at (any): the key of the member whose ancestors are returned.
/// - parent (function): returns a member key's container key, or `none` at the root.
/// - key (function): returns one member's key.
/// -> none | array
#let ancestors(members, at, parent, key: value => value) = {
  import "keys.typ": hierarchy-keys, ancestor-members

  assert(at != none, message: "ancestors `at` cannot be `none`")
  let member-keys = hierarchy-keys(members, key, "ancestors")
  if at not in member-keys { return none }
  ancestor-members(members, member-keys, at, parent, "ancestors")
}

/// Return the ancestors of the member keyed `at`, from the root down to the member itself.
///
/// Only ancestors present in `members` are included, with the member itself last. Member keys must
/// be unique, and neither they nor `at` may be `none`; an anchor absent from `members` returns
/// `none`. The walk ends when `parent` returns `none`; chains continue through keys with no member
/// in `members`, and a repeated key in the traversed chain is an error.
///
/// Example - follow a lineage through a key that is not a member:
///
/// ```typst
/// #import "@tola/collection:0.0.0": lineage
/// #let pages = ((id: "leaf", title: "Leaf"), (id: "root", title: "Root"))
/// #let parents = (leaf: "section", section: "root", root: none)
/// #let trail = lineage(pages, "leaf", key => parents.at(key), key: page => page.id)
/// #assert.eq(trail.map(page => page.title), ("Root", "Leaf"))
/// ```
///
/// Related: ancestors
/// - members (array): the members to search.
/// - at (any): the key of the member whose lineage is returned.
/// - parent (function): returns a member key's container key, or `none` at the root.
/// - key (function): returns one member's key.
/// -> none | array
#let lineage(members, at, parent, key: value => value) = {
  import "keys.typ": hierarchy-keys, ancestor-members

  assert(at != none, message: "lineage `at` cannot be `none`")
  let member-keys = hierarchy-keys(members, key, "lineage")
  let position = member-keys.position(member-key => member-key == at)
  if position == none { return none }
  let lineage = ancestor-members(members, member-keys, at, parent, "lineage")
  lineage.push(members.at(position))
  lineage
}

/// Return the members that share the anchor's parent, in input order, excluding the anchor.
///
/// Member keys must be unique, and neither they nor `at` may be `none`; an anchor absent from
/// `members` returns `none`. A member whose `parent` returns `none` is a root; roots have no
/// siblings, even though all of them share `parent: none`. Only direct parent edges are examined,
/// and a self-edge is an error.
///
/// Example - list the members sharing a parent:
///
/// ```typst
/// #import "@tola/collection:0.0.0": siblings
/// #let pages = ((id: "root"), (id: "api"), (id: "faq"), (id: "usage"))
/// #let parents = (api: "root", faq: "root", usage: "api", root: none)
/// #let peers = siblings(pages, "api", key => parents.at(key), key: page => page.id)
/// #assert.eq(peers.map(page => page.id), ("faq",))
/// ```
///
/// Example - leave roots without siblings:
///
/// ```typst
/// #import "@tola/collection:0.0.0": siblings
/// #let pages = ((id: "root"), (id: "aside"), (id: "api"))
/// #let parents = (root: none, aside: none, api: "root")
/// #assert.eq(siblings(pages, "root", key => parents.at(key), key: page => page.id), ())
/// #assert.eq(siblings(pages, "aside", key => parents.at(key), key: page => page.id), ())
/// ```
///
/// Related: children
/// - members (array): the members to search.
/// - at (any): the key of the member whose siblings are returned.
/// - parent (function): returns a member key's container key, or `none` at the root.
/// - key (function): returns one member's key.
/// -> none | array
#let siblings(members, at, parent, key: value => value) = {
  import "keys.typ": hierarchy-keys, parent-key

  assert(at != none, message: "siblings `at` cannot be `none`")
  let member-keys = hierarchy-keys(members, key, "siblings")
  if at not in member-keys { return none }
  let container = parent-key(at, parent, "siblings")
  if container == none { return () }
  let siblings = ()
  for (position, member-key) in member-keys.enumerate() {
    if member-key != at and parent-key(member-key, parent, "siblings") == container {
      siblings.push(members.at(position))
    }
  }
  siblings
}

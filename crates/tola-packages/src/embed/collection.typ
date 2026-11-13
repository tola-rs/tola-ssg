// @tola/collection:0.0.0 - query, group, filter, and walk arrays of your own values
//
// Four kinds of operations on one array of members — the values you supply:
//
// - Index and group: `index-by`, `group-by`, `group-by-keys`
// - Filter and copy fields: `select-members`, `pick`
// - Order and neighbors: `adjacent`, `window`
// - Hierarchy: `children`, `descendants`, `ancestors`, `lineage`, `siblings`
//
// Supply `key` to get a member's key, `keys` to get all its group keys, and `parent`
// to look up the parent key from a child's key. For a root member, return `none`
// from `parent`. To find children or descendants, you can name a parent key that
// is absent from the array. A query that needs the member itself returns `none`
// when the member is absent. A search with no matches returns an empty array.
//
// Selections keep the order in `members`. `ancestors` and `lineage` start at the root.
// Groups follow the order in which their keys first appear. Member keys must be
// unique for `index-by`, `adjacent`, `window`, and the hierarchy functions.
// `group-by`, `group-by-keys`, and `select-members` allow duplicates.

/// Return a new dictionary with the entries of `fields` named by `keys`, in the order you list them.
///
/// Use string keys. A key such as `"a.b"` names one entry. Only keys present in `fields`
/// are copied, and each key is copied once. Values stay unchanged, including `none`.
///
/// Example: Copy selected fields, including a dotted key:
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
  import "keys.typ": require

  require(type(fields) == dictionary, message: "pick `fields` must be a dictionary; found `" + repr(type(fields)) + "`")
  require(type(keys) == array, message: "pick `keys` must be an array of strings; found `" + repr(type(keys)) + "`")
  let selected = (:)
  for (index, field) in keys.enumerate() {
    require(
      type(field) == str,
      message: "pick `keys` must contain strings; found `" + repr(type(field)) + "` at index " + str(index),
    )
    if field in fields and field not in selected {
      selected.insert(field, fields.at(field))
    }
  }
  selected
}

/// Index the members by one key each: the result maps each key to its member.
///
/// The function calls `key` once per member. Two members sharing a key cause an error.
///
/// Example: Index members by path:
///
/// ```typst
/// #import "@tola/collection:0.0.0": index-by
/// #let members = (
///   (path: "index.html", title: "Home"),
///   (path: "404.html", title: "Not Found"),
/// )
/// #let by-path = index-by(members, key: member => member.path)
/// #assert.eq(by-path.at("404.html"), members.at(1))
/// #assert.eq(by-path.at("index.html"), members.at(0))
/// ```
///
/// Related: group-by
/// - members (array): the members to index.
/// - key (function): return a string key for each member. If omitted, each member must itself be a string.
/// -> dictionary
#let index-by(members, key: value => value) = {
  import "keys.typ": duplicate-key-message, require

  require(type(members) == array, message: "index-by `members` must be an array; found `" + repr(type(members)) + "`")
  let by-key = (:)
  for (index, member) in members.enumerate() {
    let member-key = key(member)
    require(
      type(member-key) == str,
      message: "index-by `key` must return a string; found `"
        + repr(type(member-key))
        + "` for member at index "
        + str(index),
    )
    if member-key in by-key {
      require(false, message: duplicate-key-message("index-by", member-key))
    }
    by-key.insert(member-key, member)
  }
  by-key
}

/// Group the members by one key each: every member lands in exactly one group.
/// To group a member under several keys, use `group-by-keys`.
///
/// The function calls `key` once per member. Groups appear in the order
/// their keys first appear, and members keep input order.
///
/// Example: Group members by one key each:
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
/// - key (function): return a string key for each member. If omitted, each member must itself be a string.
/// -> dictionary
#let group-by(members, key: value => value) = {
  import "keys.typ": require

  require(type(members) == array, message: "group-by `members` must be an array; found `" + repr(type(members)) + "`")
  let groups = (:)
  for (index, member) in members.enumerate() {
    let group-key = key(member)
    require(
      type(group-key) == str,
      message: "group-by `key` must return a string; found `"
        + repr(type(group-key))
        + "` for member at index "
        + str(index),
    )
    if group-key not in groups { groups.insert(group-key, ()) }
    groups.at(group-key).push(member)
  }
  groups
}

/// Group each member under all of its keys, for example a post under each of its tags.
/// Use `group-by` when each member has only one key.
///
/// Tola calls `keys` once per member. Repeated keys count once. A member with no keys joins no
/// group. Groups appear in the order their keys first appear, and members keep input order.
///
/// Example: Group notes by tag:
///
/// ```typst
/// #import "@tola/collection:0.0.0": group-by-keys
/// #let notes = (
///   (id: "A", tags: ("web", "typst")),
///   (id: "B", tags: ("cli",)),
///   (id: "C", tags: ("design", "typst")),
///   (id: "D", tags: ()),
/// )
/// #let groups = group-by-keys(notes, keys: note => note.tags)
/// #assert.eq(groups.keys(), ("web", "typst", "cli", "design"))
/// #assert.eq(groups.at("web").map(note => note.id), ("A",))
/// #assert.eq(groups.at("typst").map(note => note.id), ("A", "C"))
/// #assert.eq(groups.at("cli").map(note => note.id), ("B",))
/// ```
///
/// Related: group-by
/// - members (array): the members to group.
/// - keys (function): return an array of group keys. If omitted, each member must be an array of strings.
/// -> dictionary
#let group-by-keys(members, keys: value => value) = {
  import "keys.typ": memberships-of, require

  require(
    type(members) == array,
    message: "group-by-keys `members` must be an array; found `" + repr(type(members)) + "`",
  )
  let groups = (:)
  for member in members {
    for group-key in memberships-of(member, keys, "group-by-keys").dedup() {
      require(
        type(group-key) == str,
        message: "group-by-keys `keys` must return string keys; found `" + repr(type(group-key)) + "`",
      )
      if group-key not in groups { groups.insert(group-key, ()) }
      groups.at(group-key).push(member)
    }
  }
  groups
}

/// Filter members by keys. For example, use `keys: note => note.tags` to find notes by tag.
///
/// `"any"` matches at least one key in `wanted`. `"all"` matches every key in `wanted`.
/// `(none,)` supplies one key and `()` supplies none. With an empty `wanted`, `"any"`
/// selects nothing and `"all"` selects every member.
///
/// Example: Match any or all memberships:
///
/// ```typst
/// #import "@tola/collection:0.0.0": select-members
/// #let notes = (
///   (id: "A", tags: ("web", "typst")),
///   (id: "B", tags: ("cli", "web")),
///   (id: "C", tags: ()),
/// )
/// #assert.eq(select-members(notes, ("web",), keys: note => note.tags).map(note => note.id), ("A", "B"))
/// #assert.eq(
///   select-members(notes, ("web", "typst"), keys: note => note.tags, match: "all").map(note => note.id),
///   ("A",),
/// )
/// ```
///
/// Example: Match with no requested keys:
///
/// ```typst
/// #import "@tola/collection:0.0.0": select-members
/// #let notes = ((id: "A", tags: ("web",)), (id: "B", tags: ("typst",)))
/// #assert.eq(select-members(notes, (), keys: note => note.tags), ())
/// #assert.eq(select-members(notes, (), keys: note => note.tags, match: "all"), notes)
/// ```
///
/// Related: group-by-keys
/// - members (array): the members to filter.
/// - wanted (array): the memberships a member must have.
/// - keys (function): return an array of keys to match. Keys may include `none`. If omitted, each member must be an array.
/// - match ("any" | "all"): `"any"` (the default) matches at least one wanted key. `"all"` requires every wanted key.
/// -> array
#let select-members(members, wanted, keys: value => value, match: "any") = {
  import "keys.typ": memberships-of, require

  require(
    type(members) == array,
    message: "select-members `members` must be an array; found `" + repr(type(members)) + "`",
  )
  require(
    type(wanted) == array,
    message: "select-members `wanted` must be an array; found `" + repr(type(wanted)) + "`",
  )
  require(
    match in ("any", "all"),
    message: "select-members `match` must be `\"any\"` or `\"all\"`; found `" + repr(match) + "`",
  )
  members.filter(member => {
    let available = memberships-of(member, keys, "select-members")
    if match == "all" {
      wanted.all(value => value in available)
    } else {
      wanted.any(value => value in available)
    }
  })
}

/// Return the neighbors of the member keyed `at`, as `{ before, after }`.
///
/// If no member has the key `at`, return `none`. A first member has no `before` entry,
/// and a last member has no `after` entry. A neighbor that exists can have the value `none`.
///
/// Member keys must be unique. Any comparable value, including `none`, can be a key.
///
/// Example: Find a member's neighbors:
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
/// Example: Find neighbors at either end of a list:
///
/// ```typst
/// #import "@tola/collection:0.0.0": adjacent
/// #let steps = (none, "middle", "last")
/// #assert.eq(adjacent(steps, none), (after: "middle"))
/// #assert.eq(adjacent(steps, "last"), (before: "middle"))
/// #assert.eq(adjacent(steps, "middle"), (before: none, after: "last"))
/// ```
///
/// Example: Link to the previous and next page:
///
/// In a site created with `tola init`, save this as `site.typ` and run `tola dev`.
/// Open `/guide/install/` on the printed server URL. It links back to Guide and forward
/// to Deploy. The array sets the reading order.
///
/// ```typst
/// #import "@tola/address:0.0.0": output-url
/// #import "@tola/collection:0.0.0": adjacent
///
/// #let pages = (
///   (output: "guide/index.html", title: "Guide"),
///   (output: "guide/install/index.html", title: "Install"),
///   (output: "guide/deploy/index.html", title: "Deploy"),
/// )
///
/// #for page in pages {
///   let neighbors = adjacent(pages, page.output, key: item => item.output)
///   document(page.output, title: page.title)[
///     #heading(level: 1, page.title)
///     #for direction in ("before", "after") {
///       if direction in neighbors {
///         let item = neighbors.at(direction)
///         block(link(output-url(item.output), item.title))
///       }
///     }
///   ]
/// }
/// ```
///
/// Related: window, index-by
/// - members (array): the members to search.
/// - at (any): the key whose neighbors are returned.
/// - key (function): return a key for each member. If omitted, use the member itself as its key.
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

/// Return up to `before` preceding and `after` following members around the anchor, excluding it.
///
/// The result keeps input order. If no member has the key `at`, return `none`.
/// Member keys must be unique.
///
/// Example: Collect a window around a member:
///
/// ```typst
/// #import "@tola/collection:0.0.0": window
/// #let pages = ((id: "a"), (id: "b"), (id: "c"), (id: "d"))
/// #let around = window(pages, "c", key: page => page.id, before: 1, after: 1)
/// #assert.eq(around.map(page => page.id), ("b", "d"))
/// #assert.eq(window(pages, "z", key: page => page.id), none)
/// ```
///
/// Example: Keep one side of the anchor:
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
/// - key (function): return a key for each member. If omitted, use the member itself as its key.
/// - before (int): Number of preceding members to include. Use a nonnegative integer. If omitted, include none.
/// - after (int): Number of following members to include. Use a nonnegative integer. If omitted, include none.
/// -> none | array
#let window(members, at, key: value => value, before: 0, after: 0) = {
  import "keys.typ": position-of, require

  require(
    type(before) == int and before >= 0,
    message: "window `before` must be a nonnegative integer; found `" + repr(before) + "`",
  )
  require(
    type(after) == int and after >= 0,
    message: "window `after` must be a nonnegative integer; found `" + repr(after) + "`",
  )
  let position = position-of(members, at, key, "window")
  if position == none { return none }
  let leading = members.slice(calc.max(0, position - before), position)
  let trailing = members.slice(position + 1, count: calc.min(after, members.len() - position - 1))
  leading + trailing
}

/// Return the direct children of the container key `of`.
///
/// The parent named by `of` can be absent from `members`. Neither `of` nor a member key can
/// be `none`. Each member must have a unique key. A member cannot be its own parent.
///
/// Example: List a container's direct children:
///
/// ```typst
/// #import "@tola/collection:0.0.0": children
/// #let pages = ((id: "root"), (id: "api"), (id: "usage"), (id: "faq"))
/// #let parents = (api: "root", usage: "api", faq: "root", root: none)
/// #let kids = children(pages, "root", key => parents.at(key), key: page => page.id)
/// #assert.eq(kids.map(page => page.id), ("api", "faq"))
/// ```
///
/// Example: List children of a container absent from members:
///
/// ```typst
/// #import "@tola/collection:0.0.0": children
/// #let pages = ((id: "intro"), (id: "api"))
/// #let parents = (intro: "guide", api: "guide")
/// #let ids = children(pages, "guide", key => parents.at(key), key: page => page.id)
/// #assert.eq(ids.map(page => page.id), ("intro", "api"))
/// ```
///
/// Related: descendants
/// - members (array): the members to search.
/// - of (any): the container key whose direct children are returned.
/// - parent (function): receives a member key rather than the member; returns its container key,
///   or `none` at the root.
/// - key (function): return a key for each member. If omitted, use the member itself as its key.
/// -> array
#let children(members, of, parent, key: value => value) = {
  import "keys.typ": hierarchy-keys, parent-key, require

  require(of != none, message: "children `of` cannot be `none`")
  let member-keys = hierarchy-keys(members, key, "children")
  let children = ()
  for (position, member-key) in member-keys.enumerate() {
    if parent-key(member-key, parent, "children") == of {
      children.push(members.at(position))
    }
  }
  children
}

/// Return every descendant of the container key `of`, excluding the container.
///
/// The parent named by `of` can be absent from `members`. Neither `of` nor a member key can
/// be `none`. Each member must have a unique key.
///
/// Follow `parent` from each member to the root, including through keys absent from `members`.
/// The function checks the entire path for cycles, even after it finds `of`.
///
/// Example: List every descendant of a container:
///
/// ```typst
/// #import "@tola/collection:0.0.0": descendants
/// #let pages = ((id: "root"), (id: "api"), (id: "usage"), (id: "faq"))
/// #let parents = (api: "root", usage: "api", faq: "root", root: none)
/// #let nested = descendants(pages, "root", key => parents.at(key), key: page => page.id)
/// #assert.eq(nested.map(page => page.id), ("api", "usage", "faq"))
/// ```
///
/// Example: Follow parent keys through an absent member:
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
/// - key (function): return a key for each member. If omitted, use the member itself as its key.
/// -> array
#let descendants(members, of, parent, key: value => value) = {
  import "keys.typ": hierarchy-keys, parent-chain, require

  require(of != none, message: "descendants `of` cannot be `none`")
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
/// If no member has the key `at`, return `none`. Neither `at` nor a member key can be `none`.
/// Each member must have a unique key.
///
/// Follow `parent` until it returns `none`. Keys absent from `members` can lead to other
/// ancestors, but do not appear in the result. A repeated key on the path is an error.
///
/// Example: Walk a member's ancestor chain:
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
/// - key (function): return a key for each member. If omitted, use the member itself as its key.
/// -> none | array
#let ancestors(members, at, parent, key: value => value) = {
  import "keys.typ": ancestor-members, hierarchy-keys, require

  require(at != none, message: "ancestors `at` cannot be `none`")
  let member-keys = hierarchy-keys(members, key, "ancestors")
  if at not in member-keys { return none }
  ancestor-members(members, member-keys, at, parent, "ancestors")
}

/// Return the ancestors of the member keyed `at`, with `at` itself last — a breadcrumb trail.
///
/// If no member has the key `at`, return `none`. Neither `at` nor a member key can be `none`.
/// Each member must have a unique key.
///
/// Follow `parent` until it returns `none`. Keys absent from `members` can lead to other
/// ancestors, but do not appear in the result. A repeated key on the path is an error.
///
/// Example: Trace a lineage through an absent member:
///
/// ```typst
/// #import "@tola/collection:0.0.0": lineage
/// #let pages = ((id: "leaf", title: "Leaf"), (id: "root", title: "Root"))
/// #let parents = (leaf: "section", section: "root", root: none)
/// #let trail = lineage(pages, "leaf", key => parents.at(key), key: page => page.id)
/// #assert.eq(trail.map(page => page.title), ("Root", "Leaf"))
/// ```
///
/// Example: Look up an absent anchor:
///
/// ```typst
/// #import "@tola/collection:0.0.0": lineage
/// #let pages = ((id: "root", title: "Root"),)
/// #assert.eq(lineage(pages, "missing", key => none, key: page => page.id), none)
/// ```
///
/// Related: ancestors
/// - members (array): the members to search.
/// - at (any): the key of the member whose lineage is returned.
/// - parent (function): returns a member key's container key, or `none` at the root.
/// - key (function): return a key for each member. If omitted, use the member itself as its key.
/// -> none | array
#let lineage(members, at, parent, key: value => value) = {
  import "keys.typ": ancestor-members, hierarchy-keys, require

  require(at != none, message: "lineage `at` cannot be `none`")
  let member-keys = hierarchy-keys(members, key, "lineage")
  let position = member-keys.position(member-key => member-key == at)
  if position == none { return none }
  let lineage = ancestor-members(members, member-keys, at, parent, "lineage")
  lineage.push(members.at(position))
  lineage
}

/// Return the other members sharing the parent of the member keyed `at`.
///
/// If no member has the key `at`, return `none`. If its `parent` returns `none`, it is a root
/// and the result is `()`.
///
/// Neither `at` nor a member key can be `none`. Each member must have a unique key.
/// A member cannot be its own parent.
///
/// Example: List the members sharing a parent:
///
/// ```typst
/// #import "@tola/collection:0.0.0": siblings
/// #let pages = ((id: "root"), (id: "api"), (id: "faq"), (id: "usage"))
/// #let parents = (api: "root", faq: "root", usage: "api", root: none)
/// #let peers = siblings(pages, "api", key => parents.at(key), key: page => page.id)
/// #assert.eq(peers.map(page => page.id), ("faq",))
/// ```
///
/// Example: Check that root members have no siblings:
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
/// - key (function): return a key for each member. If omitted, use the member itself as its key.
/// -> none | array
#let siblings(members, at, parent, key: value => value) = {
  import "keys.typ": hierarchy-keys, parent-key, require

  require(at != none, message: "siblings `at` cannot be `none`")
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

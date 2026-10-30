// @tola/graph:0.0.0 - directed graphs over values you supply
//
// Build one graph from two arrays — nodes and edges — then inspect it, walk it, and narrow it:
//
// - Build: `from-records`
// - Inspect: `node-keys`, `has-node`, `node-data`, `edges`, `in-edges`, `out-edges`
// - Walk: `reachable`
// - Narrow: `select-edges`, `induced`
//
// A node's key is the string its extractor returns; the default is the value itself, so string
// nodes need no extractor. Keys are unique, and the empty string is a key like any other. Edges
// may be parallel, cyclic, or self-referential, and a node with no edge is still a node. Every
// result keeps the order you gave: nodes in registration order, edges in input order, and the
// values you supplied come back unchanged. An edge record also carries the `index` it had in the
// graph it came from; that index identifies the edge inside this graph and the selections made
// from it, not across constructions.
//
// Read a graph only through the functions below; its fields are private.

/// Build a graph from node values and edge values.
///
/// `node-key` reads a node's key, and `source` and `target` read an edge's endpoints. The
/// defaults take a string node as its own key and an edge's `source` and `target` fields, so
/// your own records can carry anything else.
///
/// Constraints: `nodes` and `edges` must be arrays; each extractor runs once per input value;
/// `node-key` must return a unique string; every edge endpoint must name a registered node.
///
/// Example - build a graph and read it back:
///
/// ```typst
/// #import "@tola/graph:0.0.0" as graph
/// #let g = graph.from-records(("home", "guide", "install"), (
///   (source: "home", target: "guide"),
///   (source: "guide", target: "install"),
/// ))
/// #assert.eq(graph.node-keys(g), ("home", "guide", "install"))
/// #assert.eq(graph.reachable(g, ("home",)), ("home", "guide", "install"))
/// ```
///
/// Example - use records of your own and name their fields:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, edges
/// #let pages = ((id: "home", title: "Home"), (id: "about", title: "About"))
/// #let g = from-records(
///   pages,
///   ((from: "home", to: "about"),),
///   node-key: page => page.id,
///   source: edge => edge.from,
///   target: edge => edge.to,
/// )
/// #assert.eq(edges(g).at(0).data.to, "about")
/// ```
///
/// Related: node-keys, edges
/// - nodes (array): node values in registration order.
/// - edges (array): edge values in input order.
/// - node-key (function): receives one node; returns its string key.
/// - source (function): receives one edge; returns its source key.
/// - target (function): receives one edge; returns its target key.
/// -> dictionary
#let from-records(nodes, edges, node-key: value => value, source: edge => edge.source, target: edge => edge.target) = {
  import "keys.typ": require
  import "graph-internal.typ": make-graph
  require(type(nodes) == array, message: "from-records `nodes` must be an array")
  require(type(edges) == array, message: "from-records `edges` must be an array")
  let by-key = (:)
  for (index, node) in nodes.enumerate() {
    let key = node-key(node)
    require(type(key) == str, message: "from-records `node-key` must return a string for node at index " + str(index))
    require(key not in by-key, message: "from-records duplicate node key " + repr(key))
    by-key.insert(key, node)
  }
  let normalized = ()
  for (index, edge) in edges.enumerate() {
    let from = source(edge)
    let to = target(edge)
    require(type(from) == str, message: "from-records source of edge at index " + str(index) + " must be a string")
    require(type(to) == str, message: "from-records target of edge at index " + str(index) + " must be a string")
    require(from in by-key, message: "from-records edge at index " + str(index) + " has unknown source " + repr(from))
    require(to in by-key, message: "from-records edge at index " + str(index) + " has unknown target " + repr(to))
    normalized.push((index: index, source: from, target: to, data: edge))
  }
  make-graph(by-key, normalized)
}

/// Return every node key in registration order, isolated nodes included.
///
/// Example - a node with no edge still appears:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, node-keys
/// #let g = from-records(("a", "b", "c"), ((source: "a", target: "b"),))
/// #assert.eq(node-keys(g), ("a", "b", "c"))
/// ```
///
/// Related: has-node, node-data
/// - graph (dictionary): the graph to read.
/// -> array
#let node-keys(graph) = graph.nodes.keys()

/// Return whether `key` is a registered node.
///
/// The key must be the exact string, and a node whose value is `none` still counts as registered.
///
/// Constraints: a non-string key is an error.
///
/// Example - tell a registered node from a missing one:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, has-node
/// #let g = from-records((none,), (), node-key: _ => "placeholder")
/// #assert.eq(has-node(g, "placeholder"), true)
/// #assert.eq(has-node(g, "missing"), false)
/// ```
///
/// Related: node-data, node-keys
/// - graph (dictionary): the graph to read.
/// - key (str): the exact node key.
/// -> bool
#let has-node(graph, key) = {
  import "keys.typ": require
  require(type(key) == str, message: "has-node `key` must be a string")
  key in graph.nodes
}

/// Return the value you supplied for the node registered under `key`.
///
/// Constraints: `key` must be a string and name a registered node; an unknown key is an error.
///
/// Example - read the record a node carries:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, node-data
/// #let pages = ((path: "index.html", title: "Home"),)
/// #let g = from-records(pages, (), node-key: page => page.path)
/// #assert.eq(node-data(g, "index.html").title, "Home")
/// ```
///
/// Related: has-node, node-keys
/// - graph (dictionary): the graph to read.
/// - key (str): the exact node key.
/// -> any
#let node-data(graph, key) = {
  import "graph-internal.typ": require-node
  require-node(graph, key, "node-data")
  graph.nodes.at(key)
}

/// Return every edge record in input order.
///
/// Each record carries `index`, `source`, `target`, and the edge value you supplied in `data`.
/// The index identifies the edge within this graph and the selections made from it; it means
/// nothing after another construction.
///
/// Example - read a record and its original value:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, edges
/// #let g = from-records(("a", "b"), ((weight: 2, source: "a", target: "b"),))
/// #assert.eq(edges(g).map(edge => edge.index), (0,))
/// #assert.eq(edges(g).at(0).data.weight, 2)
/// ```
///
/// Related: in-edges, out-edges, select-edges
/// - graph (dictionary): the graph to read.
/// -> array
#let edges(graph) = graph.edges

/// Return the edge records whose target is `key`, in input order.
///
/// Constraints: `key` must be a string and name a registered node; an unknown key is an error.
///
/// Example - ask what links to a page:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, in-edges
/// #let g = from-records(("a", "b", "c"), (
///   (source: "a", target: "c"),
///   (source: "b", target: "c"),
///   (source: "c", target: "a"),
/// ))
/// #assert.eq(in-edges(g, "c").map(edge => edge.source), ("a", "b"))
/// #assert.eq(in-edges(g, "b"), ())
/// ```
///
/// Related: out-edges, edges, reachable
/// - graph (dictionary): the graph to read.
/// - key (str): the exact node key.
/// -> array
#let in-edges(graph, key) = {
  import "graph-internal.typ": require-node
  require-node(graph, key, "in-edges")
  graph.incoming.at(key)
}

/// Return the edge records whose source is `key`, in input order.
///
/// Constraints: `key` must be a string and name a registered node; an unknown key is an error.
///
/// Example - ask where a page links:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, out-edges
/// #let g = from-records(("a", "b", "c"), (
///   (source: "a", target: "b"),
///   (source: "a", target: "c"),
///   (source: "b", target: "c"),
/// ))
/// #assert.eq(out-edges(g, "a").map(edge => edge.target), ("b", "c"))
/// #assert.eq(out-edges(g, "c"), ())
/// ```
///
/// Related: in-edges, edges, reachable
/// - graph (dictionary): the graph to read.
/// - key (str): the exact node key.
/// -> array
#let out-edges(graph, key) = {
  import "graph-internal.typ": require-node
  require-node(graph, key, "out-edges")
  graph.outgoing.at(key)
}

/// Return the nodes you can reach from `roots`, in registration order, roots included.
///
/// Repeated roots count once. `"out"` follows edges forward, from source to target; `"in"`
/// follows them back, from target to source. A root reaches itself even when no edge touches it,
/// and no roots means no nodes.
///
/// Constraints: every root must be a registered string key; an unknown root is an error.
///
/// Example - walk forward from one page:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, reachable
/// #let g = from-records(("home", "guide", "install", "about"), (
///   (source: "home", target: "guide"),
///   (source: "guide", target: "install"),
/// ))
/// #assert.eq(reachable(g, ("home",)), ("home", "guide", "install"))
/// #assert.eq(reachable(g, ("about",)), ("about",))
/// ```
///
/// Example - walk backward to find everything that links into a page:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, reachable
/// #let g = from-records(("home", "guide", "install"), (
///   (source: "home", target: "guide"),
///   (source: "guide", target: "install"),
/// ))
/// #assert.eq(reachable(g, ("install",), direction: "in"), ("home", "guide", "install"))
/// ```
///
/// Related: in-edges, out-edges
/// - graph (dictionary): the graph to read.
/// - roots (array): registered string node keys.
/// - direction (str): `"out"` follows edges forward, `"in"` follows them back; defaults to
///   `"out"`.
/// -> array
#let reachable(graph, roots, direction: "out") = {
  import "keys.typ": require
  import "graph-internal.typ": require-node
  require(type(roots) == array, message: "reachable `roots` must be an array")
  require(direction in ("out", "in"), message: "reachable `direction` must be \"out\" or \"in\"")
  let seen = (:)
  let pending = ()
  for key in roots {
    require-node(graph, key, "reachable")
    if key not in seen {
      seen.insert(key, true)
      pending.push(key)
    }
  }
  let position = 0
  while position < pending.len() {
    let key = pending.at(position)
    let adjacent = if direction == "out" { graph.outgoing.at(key) } else { graph.incoming.at(key) }
    for edge in adjacent {
      let next = if direction == "out" { edge.target } else { edge.source }
      if next not in seen {
        seen.insert(next, true)
        pending.push(next)
      }
    }
    position += 1
  }
  graph.nodes.keys().filter(key => key in seen)
}

/// Keep the edges for which `keep` returns `true`.
///
/// `keep` receives one edge record at a time. All nodes remain, even when no edge does, and the
/// surviving edges keep their original `index` and relative order, so gaps are normal.
///
/// Constraints: `keep` must return a bool; anything else is an error.
///
/// Example - keep only the edges into a page:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, select-edges, edges, node-keys
/// #let g = from-records(("a", "b", "c"), (
///   (source: "a", target: "b"),
///   (source: "b", target: "c"),
///   (source: "a", target: "c"),
/// ))
/// #let into-c = select-edges(g, edge => edge.target == "c")
/// #assert.eq(edges(into-c).map(edge => edge.index), (1, 2))
/// #assert.eq(node-keys(into-c), ("a", "b", "c"))
/// ```
///
/// Related: induced, edges
/// - graph (dictionary): the graph to read.
/// - keep (function): receives one edge record; returns a bool.
/// -> dictionary
#let select-edges(graph, keep) = {
  import "keys.typ": require
  import "graph-internal.typ": make-graph
  let selected = ()
  for edge in graph.edges {
    let kept = keep(edge)
    require(type(kept) == bool, message: "select-edges `keep` must return a bool")
    if kept { selected.push(edge) }
  }
  make-graph(graph.nodes, selected)
}

/// Keep the nodes named by `keys`, and the edges whose two endpoints are among them.
///
/// Repeated keys count once. Nodes keep their registration order, edges their input order, and
/// your values stay unchanged; an edge with one endpoint outside `keys` is dropped.
///
/// Constraints: every key must be a registered string node key; an unknown key is an error.
///
/// Example - take a section and only the links inside it:
///
/// ```typst
/// #import "@tola/graph:0.0.0": from-records, induced, edges, node-keys
/// #let g = from-records(("guide", "install", "deploy", "blog"), (
///   (source: "guide", target: "install"),
///   (source: "install", target: "blog"),
///   (source: "deploy", target: "install"),
/// ))
/// #let section = induced(g, ("deploy", "install", "guide"))
/// #assert.eq(node-keys(section), ("guide", "install", "deploy"))
/// #assert.eq(edges(section).map(edge => edge.index), (0, 2))
/// ```
///
/// Related: select-edges
/// - graph (dictionary): the graph to read.
/// - keys (array): registered string node keys.
/// -> dictionary
#let induced(graph, keys) = {
  import "keys.typ": require
  import "graph-internal.typ": make-graph, require-node
  require(type(keys) == array, message: "induced `keys` must be an array")
  let wanted = (:)
  for key in keys {
    require-node(graph, key, "induced")
    wanted.insert(key, true)
  }
  let nodes = (:)
  for (key, value) in graph.nodes.pairs() {
    if key in wanted { nodes.insert(key, value) }
  }
  let edges = graph.edges.filter(edge => edge.source in wanted and edge.target in wanted)
  make-graph(nodes, edges)
}

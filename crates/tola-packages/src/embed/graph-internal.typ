#let make-graph(nodes, edges) = {
  let incoming = (:)
  let outgoing = (:)
  for key in nodes.keys() {
    incoming.insert(key, ())
    outgoing.insert(key, ())
  }
  for edge in edges {
    incoming.at(edge.target).push(edge)
    outgoing.at(edge.source).push(edge)
  }
  (nodes: nodes, edges: edges, incoming: incoming, outgoing: outgoing)
}

#let require-node(graph, key, operation) = {
  import "keys.typ": require
  require(type(key) == str, message: operation + " requires a string node key")
  require(key in graph.nodes, message: operation + " has unknown node key " + repr(key))
}

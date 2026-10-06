// `collection` imports this file inside each operation, so these helpers never become exports.

#let memberships-of(member, keys, operation) = {
  let memberships = keys(member)
  assert(type(memberships) == array, message: operation + " `keys` must return an array")
  memberships
}

#let duplicate-key-message(operation, member-key) = {
  operation + " requires unique keys; `" + repr(member-key) + "` occurs more than once"
}

#let unique-keys(members, key, operation) = {
  assert(type(members) == array, message: operation + " expects an array")
  let member-keys = members.map(key)
  let seen = ()
  for member-key in member-keys {
    if member-key in seen {
      panic(duplicate-key-message(operation, member-key))
    }
    seen.push(member-key)
  }
  member-keys
}

#let hierarchy-keys(members, key, operation) = {
  let member-keys = unique-keys(members, key, operation)
  assert(none not in member-keys, message: operation + " hierarchy keys cannot be `none`")
  member-keys
}

#let position-of(members, at, key, operation) = {
  unique-keys(members, key, operation).position(member-key => member-key == at)
}

#let parent-key(origin, parent, operation) = {
  let ancestor = parent(origin)
  if ancestor == origin {
    panic(operation + " parent chain contains a cycle at key `" + repr(origin) + "`")
  }
  ancestor
}

#let parent-chain(origin, parent, operation) = {
  let chain = ()
  let ancestor = parent-key(origin, parent, operation)
  while ancestor != none {
    if ancestor == origin or ancestor in chain {
      panic(operation + " parent chain contains a cycle at key `" + repr(ancestor) + "`")
    }
    chain.push(ancestor)
    ancestor = parent-key(ancestor, parent, operation)
  }
  chain
}

#let ancestor-members(members, member-keys, at, parent, operation) = {
  let ancestors = ()
  for ancestor in parent-chain(at, parent, operation).rev() {
    let position = member-keys.position(member-key => member-key == ancestor)
    if position != none { ancestors.push(members.at(position)) }
  }
  ancestors
}

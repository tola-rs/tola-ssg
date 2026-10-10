#let core = {
  let schema-marker = () => none
  let declaration(kind, ..properties) = (
    marker: schema-marker,
    kind: kind,
    ..properties.named(),
  )
  let assert-schema(candidate, owner) = {
    assert(
      type(candidate) == type or (
        type(candidate) == dictionary
          and candidate.at("marker", default: none) == schema-marker
          and candidate.at("kind", default: none) in (
            "any", "object", "optional", "nullable", "literal", "enum",
            "array", "dictionary", "many", "tuple", "lazy", "union",
            "variant", "trim", "check", "convert", "describe",
          )
      ),
      message: owner + ": expected a Typst type or a schema declaration",
    )
  }
  let assert-function(callback, owner) = {
    assert(type(callback) == function, message: owner + ": expected a function")
  }
  // A Typst type value is callable (`str(7) == "7"`), so a converter may name the value type directly.
  let assert-converter(converter, owner) = {
    assert(
      type(converter) == function or type(converter) == type,
      message: owner + ": expected a function or a Typst type",
    )
  }
  let assert-positional(arguments, owner) = {
    assert(arguments.named().len() == 0, message: owner + ": unexpected named arguments")
  }
  let assert-path(path) = {
    assert(type(path) == array, message: "issue: `path` must be an array")
    for segment in path {
      assert(
        type(segment) == str or (type(segment) == int and segment >= 0),
        message: "issue: path segments must be string keys or nonnegative integer indices",
      )
    }
  }
  let issue(message, code: "custom", path: (), notes: ()) = {
    assert(type(message) == str, message: "issue: `message` must be a string")
    assert(type(code) == str, message: "issue: `code` must be a string")
    assert-path(path)
    assert(
      type(notes) == array and notes.all(note => type(note) == str),
      message: "issue: `notes` must be an array of strings",
    )
    (code: code, path: path, message: message, notes: notes, children: ())
  }
  let assert-issue(problem) = {
    assert(
      type(problem) == dictionary and problem.len() == 5
        and ("code", "path", "message", "notes", "children").all(key => key in problem),
      message: "expected an Issue with exactly `code`, `path`, `message`, `notes`, and `children`",
    )
    assert(type(problem.code) == str, message: "Issue `code` must be a string")
    assert(type(problem.message) == str, message: "Issue `message` must be a string")
    assert-path(problem.path)
    assert(
      type(problem.notes) == array and problem.notes.all(note => type(note) == str),
      message: "Issue `notes` must be an array of strings",
    )
    assert(type(problem.children) == array, message: "Issue `children` must be an array")
    for child in problem.children { assert-issue(child) }
  }
  let assert-issues(issues) = {
    assert(type(issues) == array, message: "check: expected an array of Issue values")
    for problem in issues { assert-issue(problem) }
  }
  let ok(value) = (ok: true, value: value)
  let err(..issues) = {
    assert-positional(issues, "err")
    let problems = issues.pos()
    assert(problems.len() != 0, message: "err: expected at least one Issue")
    assert-issues(problems)
    (ok: false, issues: problems)
  }
  let assert-result(converted) = {
    assert(
      type(converted) == dictionary and converted.len() == 2
        and "ok" in converted and type(converted.ok) == bool,
      message: "convert: expected `ok(value)` or `err(..issues)`",
    )
    if converted.ok {
      assert("value" in converted, message: "convert: success must contain only `ok` and `value`")
    } else {
      assert("issues" in converted, message: "convert: failure must contain only `ok` and `issues`")
      assert-issues(converted.issues)
      assert(converted.issues.len() != 0, message: "convert: failure must contain at least one Issue")
    }
  }
  let any = declaration("any")
  let schema(fields, unknown: "error") = {
    assert(type(fields) == dictionary, message: "schema: `fields` must be a dictionary")
    assert(
      unknown == "error" or unknown == "keep",
      message: "schema: `unknown` must be `\"error\"` or `\"keep\"`",
    )
    for (key, member) in fields { assert-schema(member, "schema field `" + key + "`") }
    declaration("object", fields: fields, unknown: unknown)
  }
  let optional(member, ..options) = {
    assert-schema(member, "optional")
    assert(options.pos().len() == 0, message: "optional: pass the fallback as `default: value`")
    let named = options.named()
    assert(named.keys().all(key => key == "default"), message: "optional: only `default` is a named option")
    declaration("optional", inner: member, defaults: named)
  }
  let nullable(member) = {
    assert-schema(member, "nullable")
    declaration("nullable", inner: member)
  }
  let literal(value) = declaration("literal", value: value)
  let enum-of(values) = {
    assert(type(values) == array and values.len() != 0, message: "enum-of: expected a nonempty array")
    declaration("enum", values: values)
  }
  let array-of(member) = {
    assert-schema(member, "array-of")
    declaration("array", inner: member)
  }
  let dictionary-of(member) = {
    assert-schema(member, "dictionary-of")
    declaration("dictionary", inner: member)
  }
  let one-or-many(member) = {
    assert-schema(member, "one-or-many")
    declaration("many", inner: member)
  }
  let tuple(..members, rest: none) = {
    assert-positional(members, "tuple")
    for member in members.pos() { assert-schema(member, "tuple") }
    if rest != none { assert-schema(rest, "tuple `rest`") }
    declaration("tuple", members: members.pos(), rest: rest)
  }
  let lazy(factory) = {
    assert-function(factory, "lazy")
    declaration("lazy", factory: factory)
  }
  let union(..members) = {
    assert-positional(members, "union")
    assert(members.pos().len() != 0, message: "union: expected at least one schema")
    for member in members.pos() { assert-schema(member, "union") }
    declaration("union", members: members.pos())
  }
  let unannotated(member) = {
    if type(member) == dictionary and member.kind == "describe" {
      return unannotated(member.inner)
    }
    member
  }
  let variant(tag-key, ..branches) = {
    assert(type(tag-key) == str, message: "variant: the tag key must be a string")
    assert-positional(branches, "variant")
    assert(branches.pos().len() != 0, message: "variant: expected at least one branch")
    let tags = ()
    for branch in branches.pos() {
      assert-schema(branch, "variant")
      let branch-schema = unannotated(branch)
      assert(
        type(branch-schema) == dictionary and branch-schema.kind == "object",
        message: "variant: each branch must be an object declaration",
      )
      assert(tag-key in branch-schema.fields, message: "variant: every branch must declare its tag key")
      let tag-schema = unannotated(branch-schema.fields.at(tag-key))
      assert(
        type(tag-schema) == dictionary and tag-schema.kind == "literal",
        message: "variant: each tag must be a required `literal(...)` without a default or transformation",
      )
      assert(tag-schema.value not in tags, message: "variant: branch tag values must be distinct")
      tags.push(tag-schema.value)
    }
    declaration("variant", tag-key: tag-key, branches: branches.pos(), tags: tags)
  }
  let trim(member) = {
    assert-schema(member, "trim")
    declaration("trim", inner: member)
  }
  let object-fields(member) = {
    assert(
      type(member) == dictionary,
      message: "check: `on` requires an object declaration",
    )
    if member.kind == "object" { return member.fields }
    assert(
      member.kind in ("optional", "nullable", "describe", "check"),
      message: "check: `on` cannot cross a transformation, union, variant, or lazy boundary",
    )
    object-fields(member.inner)
  }
  let check(member, inspect, on: none) = {
    assert-schema(member, "check")
    assert-function(inspect, "check")
    if on != none {
      assert(type(on) == array, message: "check: `on` must be an array of field names")
      let fields = object-fields(member)
      let dependencies = ()
      for key in on {
        assert(type(key) == str, message: "check: dependency names must be strings")
        assert(key in fields, message: "check: dependency `" + key + "` is not a declared field")
        assert(key not in dependencies, message: "check: dependency names must be distinct")
        dependencies.push(key)
      }
    }
    declaration("check", inner: member, inspect: inspect, on: on)
  }
  let refine(member, predicate, message: none) = {
    assert-function(predicate, "refine")
    assert(type(message) == str, message: "refine: `message` must be a string")
    check(member, value => {
      let accepted = predicate(value)
      assert(type(accepted) == bool, message: "refine: the predicate must return a boolean")
      if accepted { () } else { (issue(message),) }
    })
  }
  let non-empty(member) = check(member, value => {
    if type(value) == str {
      if value != "" { return () }
    } else if type(value) == array or type(value) == dictionary {
      if value.len() != 0 { return () }
    } else {
      return (issue("expected a string, array, or dictionary", code: "schema.type"),)
    }
    (issue("must not be empty", code: "schema.non-empty"),)
  })
  let convert(member, convert-value, output: any) = {
    assert-schema(member, "convert")
    assert-converter(convert-value, "convert")
    assert-schema(output, "convert `output`")
    declaration("convert", inner: member, convert-value: convert-value, output: output)
  }
  let map(member, convert-value, output: any) = {
    assert-converter(convert-value, "map")
    convert(member, value => ok(convert-value(value)), output: output)
  }
  let describe(member, description) = {
    assert-schema(member, "describe")
    assert(type(description) == str, message: "describe: the description must be a string")
    declaration("describe", inner: member, description: description)
  }
  let output-description(kind, ..properties) = (
    kind: kind, presence: "required", description: none, ..properties.named(),
  )
  let present-description(description) = {
    description.presence = "required"
    if "default" in description { let _ = description.remove("default") }
    description
  }
  let copy-metadata(shape, metadata) = {
    shape.presence = metadata.presence
    shape.description = metadata.description
    if "default" in shape { let _ = shape.remove("default") }
    if "default" in metadata { shape.insert("default", metadata.default) }
    shape
  }
  let inspect(member) = {
    assert-schema(member, "inspect")
    if type(member) == type { return output-description("type", type: member) }
    if member.kind == "any" { return output-description("any") }
    if member.kind == "lazy" {
      return (..output-description("unknown"), presence: "unknown")
    }
    if member.kind == "literal" { return output-description("literal", value: member.value) }
    if member.kind == "enum" { return output-description("enum", values: member.values) }
    if member.kind == "object" {
      let fields = (:)
      for (key, field) in member.fields { fields.insert(key, inspect(field)) }
      return output-description("object", fields: fields, unknown: member.unknown)
    }
    if member.kind in ("array", "dictionary", "many") {
      let kind = if member.kind == "dictionary" { "dictionary" } else { "array" }
      return output-description(kind, element: present-description(inspect(member.inner)))
    }
    if member.kind == "tuple" {
      return output-description("tuple",
        members: member.members.map(position => present-description(inspect(position))),
        rest: if member.rest == none { none } else { present-description(inspect(member.rest)) },
      )
    }
    if member.kind == "union" {
      let members = member.members.map(inspect)
      let presence = if members.all(description => description.presence == "required") {
        "required"
      } else if members.any(description => description.presence == "optional") {
        "optional"
      } else {
        "unknown"
      }
      return (..output-description("union", members: members), presence: presence)
    }
    if member.kind == "variant" {
      return output-description("variant",
        tag-key: member.tag-key,
        branches: member.branches.map(inspect),
        tags: member.tags,
      )
    }
    let inner = inspect(member.inner)
    if member.kind == "optional" {
      inner = present-description(inner)
      if "default" in member.defaults {
        inner.insert("default", (value: member.defaults.default))
      } else {
        inner.presence = "optional"
      }
      return inner
    }
    if member.kind == "nullable" {
      let alternative = (..present-description(inner), description: none)
      return copy-metadata(
        output-description("union", members: (inspect(type(none)), alternative)),
        inner,
      )
    }
    if member.kind == "convert" {
      let output = inspect(member.output)
      return copy-metadata(output, (..inner, description: output.description))
    }
    if member.kind == "trim" {
      return copy-metadata(output-description("type", type: str), inner)
    }
    if member.kind == "describe" { return (..inner, description: member.description) }
    inner
  }
  let accept(value, origins: (), field-pass: none) = (
    ok: true, present: true, value: value, issues: (), origins: origins, field-pass: field-pass,
  )
  let omitted = (ok: true, present: false, issues: (), origins: (), field-pass: none)
  let reject(issues, origins: (), field-pass: none) = (
    ok: false, present: true, issues: issues, origins: origins, field-pass: field-pass,
  )
  let prefix-paths(nodes, segment) = nodes.map(node => (..node, path: (segment,) + node.path))
  let starts-with-path(path, prefix) = path.len() >= prefix.len() and path.slice(0, prefix.len()) == prefix
  let apply-origins(problem, origins) = {
    for origin in origins {
      if starts-with-path(problem.path, origin.path) {
        let relative = (..problem, path: problem.path.slice(origin.path.len()))
        let nested = apply-origins(relative, origin.children)
        return (
          ..issue(origin.message, code: origin.code, path: origin.path),
          children: (nested,),
        )
      }
    }
    if problem.children.len() == 0 { return problem }
    let descendants = origins
      .filter(origin => starts-with-path(origin.path, problem.path))
      .map(origin => (..origin, path: origin.path.slice(problem.path.len())))
    (..problem, children: problem.children.map(child => apply-origins(child, descendants)))
  }
  let trace-issues(issues, origins) = issues.map(problem => apply-origins(problem, origins))
  // A wrapper raises its issue against its own inner parse; a successful conversion's
  // `schema.output` provenance describes the conversion's output schema, not that issue.
  // An optional default can nest conversion provenance below its own origin.
  let without-output-origin(origins) = origins
    .filter(origin => origin.code != "schema.output")
    .map(origin => (..origin, children: without-output-origin(origin.children)))
  let origin(code, message, children) = (code: code, path: (), message: message, children: children)
  let wrap-issues(issues, code, message) = ((..issue(message, code: code), children: issues),)
  let default-message = "declared default does not satisfy the schema"
  let output-message = "converted value does not satisfy the output schema"
  let from-default(parsed) = {
    let origins = (origin("schema.default", default-message, parsed.origins),)
    if parsed.ok { return (..parsed, origins: origins) }
    (
      ..parsed,
      origins: origins,
      issues: wrap-issues(parsed.issues, "schema.default", default-message),
    )
  }
  // A conversion preserves whole-value provenance, never paths into its discarded input shape.
  let carry-root-origins(origins, following) = {
    if origins.len() == 1 and origins.first().path == () {
      let root = origins.first()
      return ((..root, children: carry-root-origins(root.children, following)),)
    }
    following
  }
  let with-issues(parsed, issues) = {
    if issues.len() == 0 { return parsed }
    reject(issues, origins: parsed.origins, field-pass: parsed.field-pass)
  }
  let reports-only-absence(issues) = {
    if issues.len() != 1 { return false }
    let first = issues.first()
    if first.code != "schema.missing" or first.path != () { return false }
    // A note or child means the branch says more than the union's own `is required`.
    first.notes.len() == 0 and first.children.len() == 0
  }
  let type-issue(expected) = issue("expected `" + repr(expected) + "`", code: "schema.type")
  let evaluate(member, slot) = {
    if type(member) == type {
      if not slot.present { return reject((issue("is required", code: "schema.missing"),)) }
      if type(slot.value) != member { return reject((type-issue(member),)) }
      return accept(slot.value)
    }
    if member.kind == "optional" {
      if slot.present { return evaluate(member.inner, slot) }
      if "default" not in member.defaults { return omitted }
      return from-default(evaluate(member.inner, (present: true, value: member.defaults.default)))
    }
    if member.kind == "nullable" {
      if slot.present and slot.value == none { return accept(none) }
      return evaluate(member.inner, slot)
    }
    if member.kind == "lazy" {
      let deferred = (member.factory)()
      assert-schema(deferred, "lazy factory")
      return evaluate(deferred, slot)
    }
    if member.kind == "union" {
      let branches = ()
      for (index, alternative) in member.members.enumerate() {
        let parsed = evaluate(alternative, slot)
        if parsed.ok { return parsed }
        // A branch that fails only on the absent key restates the union's own `is required`.
        if not slot.present and reports-only-absence(parsed.issues) { continue }
        branches.push((
          ..issue("alternative " + str(index + 1) + " did not match", code: "schema.branch"),
          children: parsed.issues,
        ))
      }
      if branches.len() == 0 { return reject((issue("is required", code: "schema.missing"),)) }
      return reject(wrap-issues(branches, "schema.union", "does not match any alternative"))
    }
    if member.kind == "describe" { return evaluate(member.inner, slot) }
    if member.kind in ("trim", "check", "convert") {
      let parsed = evaluate(member.inner, slot)
      if parsed.ok and not parsed.present { return parsed }
      if member.kind == "check" {
        let checked-value = none
        if member.on == none {
          if not parsed.ok { return parsed }
          checked-value = parsed.value
        } else {
          if parsed.field-pass == none {
            if not parsed.ok { return parsed }
            return with-issues(
              parsed,
              trace-issues((type-issue(dictionary),), without-output-origin(parsed.origins)),
            )
          }
          if member.on.any(key => not parsed.field-pass.at(key).ok) { return parsed }
          // Dependencies use the completed child outcomes, even when an unrelated child failed.
          checked-value = (:)
          for key in member.on {
            let field = parsed.field-pass.at(key)
            if field.present { checked-value.insert(key, field.value) }
          }
        }
        let found = (member.inspect)(checked-value)
        assert-issues(found)
        return with-issues(
          parsed,
          parsed.issues + trace-issues(found, without-output-origin(parsed.origins)),
        )
      }
      if not parsed.ok { return parsed }
      if member.kind == "trim" {
        if type(parsed.value) != str {
          return with-issues(
            parsed,
            trace-issues((type-issue(str),), without-output-origin(parsed.origins)),
          )
        }
        return (..parsed, value: parsed.value.trim())
      }
      let converted = (member.convert-value)(parsed.value)
      assert-result(converted)
      if not converted.ok {
        return with-issues(
          parsed,
          trace-issues(converted.issues, without-output-origin(parsed.origins)),
        )
      }
      let output = evaluate(member.output, (present: true, value: converted.value))
      let output-origins = (origin("schema.output", output-message, output.origins),)
      let origins = carry-root-origins(parsed.origins, output-origins)
      if not output.ok {
        let root-origins = carry-root-origins(parsed.origins, ())
        return reject(
          trace-issues(wrap-issues(output.issues, "schema.output", output-message), root-origins),
          origins: origins,
        )
      }
      return accept(output.value, origins: origins)
    }
    if not slot.present { return reject((issue("is required", code: "schema.missing"),)) }
    let value = slot.value
    if member.kind == "any" { return accept(value) }
    if member.kind == "literal" {
      if value == member.value { return accept(value) }
      return reject((issue("expected the literal `" + repr(member.value) + "`", code: "schema.literal"),))
    }
    if member.kind == "enum" {
      if value in member.values { return accept(value) }
      return reject((issue("expected one of `" + repr(member.values) + "`", code: "schema.enum"),))
    }
    if member.kind == "variant" {
      if type(value) != dictionary { return reject((type-issue(dictionary),)) }
      if member.tag-key not in value {
        return reject((issue("is required", code: "schema.missing", path: (member.tag-key,)),))
      }
      let tag = value.at(member.tag-key)
      for (index, expected) in member.tags.enumerate() {
        if tag == expected { return evaluate(member.branches.at(index), slot) }
      }
      return reject((issue("does not select a declared variant", code: "schema.variant", path: (member.tag-key,)),))
    }
    if member.kind == "object" {
      if type(value) != dictionary { return reject((type-issue(dictionary),)) }
      let normalized = (:)
      let field-pass = (:)
      let issues = ()
      let origins = ()
      for (key, field-schema) in member.fields {
        let field-slot = if key in value { (present: true, value: value.at(key)) } else { (present: false) }
        let field = evaluate(field-schema, field-slot)
        field-pass.insert(key, field)
        issues += prefix-paths(field.issues, key)
        origins += prefix-paths(field.origins, key)
        if field.ok and field.present { normalized.insert(key, field.value) }
      }
      for (key, unknown-value) in value {
        if key not in member.fields {
          if member.unknown == "keep" {
            normalized.insert(key, unknown-value)
          } else {
            issues.push(issue("is not a declared field", code: "schema.unknown-key", path: (key,)))
          }
        }
      }
      if issues.len() != 0 { return reject(issues, origins: origins, field-pass: field-pass) }
      return accept(normalized, origins: origins, field-pass: field-pass)
    }
    if member.kind == "dictionary" {
      if type(value) != dictionary { return reject((type-issue(dictionary),)) }
      let normalized = (:)
      let issues = ()
      let origins = ()
      for (key, member-value) in value {
        let parsed = evaluate(member.inner, (present: true, value: member-value))
        issues += prefix-paths(parsed.issues, key)
        origins += prefix-paths(parsed.origins, key)
        if parsed.ok { normalized.insert(key, parsed.value) }
      }
      if issues.len() != 0 { return reject(issues, origins: origins) }
      return accept(normalized, origins: origins)
    }
    if member.kind == "many" and type(value) != array {
      let parsed = evaluate(member.inner, slot)
      if not parsed.ok { return parsed }
      return accept((parsed.value,), origins: prefix-paths(parsed.origins, 0))
    }
    if type(value) != array { return reject((type-issue(array),)) }
    let normalized = ()
    let issues = ()
    let origins = ()
    if member.kind == "tuple" {
      let fixed = member.members.len()
      if value.len() < fixed or (member.rest == none and value.len() > fixed) {
        let count = if member.rest == none { "exactly " } else { "at least " }
        issues.push(issue("expected " + count + str(fixed) + " members", code: "schema.arity"))
      }
      for (index, member-value) in value.enumerate() {
        let position-schema = if index < fixed {
          member.members.at(index)
        } else if member.rest != none {
          member.rest
        } else {
          continue
        }
        let parsed = evaluate(position-schema, (present: true, value: member-value))
        issues += prefix-paths(parsed.issues, index)
        origins += prefix-paths(parsed.origins, index)
        if parsed.ok { normalized.push(parsed.value) }
      }
    } else {
      for (index, member-value) in value.enumerate() {
        let parsed = evaluate(member.inner, (present: true, value: member-value))
        issues += prefix-paths(parsed.issues, index)
        origins += prefix-paths(parsed.origins, index)
        if parsed.ok { normalized.push(parsed.value) }
      }
    }
    if issues.len() != 0 { return reject(issues, origins: origins) }
    accept(normalized, origins: origins)
  }
  let try-parse(value, member) = {
    assert-schema(member, "try-parse")
    let parsed = evaluate(member, (present: true, value: value))
    if parsed.ok { ok(parsed.value) } else { err(..parsed.issues) }
  }
  let path-text(path) = {
    let rendered = "$"
    for segment in path {
      if type(segment) == int {
        rendered += "[" + str(segment) + "]"
      } else if segment.match(regex("^[A-Za-z_][A-Za-z0-9_-]*$")) != none {
        rendered += "." + segment
      } else {
        rendered += "[" + repr(segment).replace("`", "\\u{60}") + "]"
      }
    }
    rendered
  }
  let issue-lines(problem, base: (), provenance: none, indent: "") = {
    let path = base + problem.path
    let label = if provenance == none { "" } else { provenance + " " }
    let lines = (indent + label + "`" + path-text(path) + "`: " + problem.message,)
    for note in problem.notes { lines.push(indent + "  note: " + note) }
    let generated = problem.code == "schema.default" or problem.code == "schema.output"
    let child-provenance = if problem.code == "schema.default" {
      "declared default"
    } else if problem.code == "schema.output" {
      "converted value"
    } else {
      provenance
    }
    for child in problem.children {
      lines += issue-lines(
        child,
        base: if generated { () } else { path },
        provenance: child-provenance,
        indent: indent + "  ",
      )
    }
    lines
  }
  let format-issues(issues) = {
    assert-issues(issues)
    let lines = ()
    for problem in issues { lines += issue-lines(problem) }
    lines.join("\n", default: "")
  }
  let parse(value, member) = {
    let parsed = try-parse(value, member)
    if not parsed.ok { panic(format-issues(parsed.issues)) }
    parsed.value
  }
  import "schema-rules.typ": rules
  (
    any: any, schema: schema, optional: optional, nullable: nullable,
    literal: literal, enum-of: enum-of, array-of: array-of,
    dictionary-of: dictionary-of, one-or-many: one-or-many, tuple: tuple,
    lazy: lazy, union: union, variant: variant, trim: trim,
    non-empty: non-empty, refine: refine, check: check, map: map,
    convert: convert, describe: describe, inspect: inspect, parse: parse, try-parse: try-parse,
    ok: ok, err: err, issue: issue, format-issues: format-issues,
    ..rules(check, issue),
  )
}

/// Accept any present value, including `none`, and return it unchanged.
/// No value is rejected. An absent object key still needs `optional`.
///
/// Example - accept any present value:
/// ```typst
/// #import "@tola/schema:0.0.0": any, parse
/// #assert.eq(parse(none, any), none)
/// #assert.eq(parse((answer: 42), any), (answer: 42))
/// ```
/// Related: optional
#let any = core.any

/// Declare the fields of a dictionary, evaluated in the order you write them.
///
/// A parse through the declaration returns a normalized dictionary.
/// `fields` maps each key to a Typst type or another declaration, and `unknown` decides the fate of
/// keys you did not declare: `"error"` rejects them, `"keep"` preserves them without validating
/// their values. A missing declared key is an error unless its declaration accepts absence, and a
/// present declared value must satisfy its declaration.
///
/// Example - validate page metadata and supply a missing tag list:
/// ```typst
/// #import "@tola/schema:0.0.0": schema, parse, non-empty, trim, optional, array-of
/// #let page-schema = schema((
///   title: non-empty(trim(str)),
///   tags: optional(array-of(str), default: ()),
/// ))
/// #assert.eq(parse((title: " Notes "), page-schema), (title: "Notes", tags: ()))
/// ```
///
/// Example - keep undeclared keys with `unknown`:
/// ```typst
/// #import "@tola/schema:0.0.0": parse, schema, try-parse
/// #let page = schema((title: str), unknown: "keep")
/// #assert.eq(parse((title: "Hello", draft: true), page), (title: "Hello", draft: true))
/// #assert.eq(try-parse((title: "Hello", draft: true), schema((title: str))).ok, false)
/// ```
///
/// Related: optional, parse
/// - fields (dictionary): a Typst type or another schema declaration for each field.
/// - unknown (string): `"error"` rejects extra keys; `"keep"` preserves them without validating
///   their values.
/// -> dictionary
#let schema = core.schema

/// Allow an absent object key, and return the inner value when the key is present.
/// `default: value` supplies the value for a missing key, parsed through the inner schema only
/// when the key is missing. A present value always takes the inner schema, and fails on its own.
/// The default expression is evaluated when you construct the declaration, and a function supplied
/// as a default stays that value.
/// Without a default, a missing key stays absent in the normalized dictionary. A present `none`,
/// `auto`, empty string, or empty array is a value and reaches the inner schema, so
/// `optional(nullable(str))` accepts absence or `none` while `optional(str)` accepts absence or a
/// string. Optional array and tuple members still occupy a required position.
///
/// Example - supply a default for a missing key:
/// ```typst
/// #import "@tola/schema:0.0.0": schema, optional, parse
/// #let settings = schema((retries: optional(int, default: 3)))
/// #assert.eq(parse((:), settings), (retries: 3))
/// ```
///
/// Example - tell an absent key from an explicit `none`:
/// ```typst
/// #import "@tola/schema:0.0.0": nullable, optional, parse, schema, try-parse
/// #let skippable = schema((nickname: optional(str)))
/// #assert.eq(parse((:), skippable), (:))
/// #assert.eq(try-parse((nickname: none), skippable).ok, false)
/// #let explicit = schema((nickname: optional(nullable(str))))
/// #assert.eq(parse((nickname: none), explicit), (nickname: none))
/// ```
///
/// Related: nullable, schema
/// - member (type | dictionary): the schema a present value reaches and an absent key skips.
/// -> dictionary
#let optional = core.optional

/// Accept an explicit `none` before consulting the inner schema.
/// Any other value, including an absent object key, reaches the inner schema.
///
/// Example - accept an explicit none:
/// ```typst
/// #import "@tola/schema:0.0.0": schema, optional, nullable, parse
/// #let profile = schema((nickname: optional(nullable(str))))
/// #assert.eq(parse((nickname: none), profile), (nickname: none))
/// #assert.eq(parse((:), profile), (:))
/// ```
///
/// Related: optional
/// - member (type | dictionary): the schema every value other than an explicit `none` reaches.
/// -> dictionary
#let nullable = core.nullable

/// Accept a value equal to the declared Typst value, and return it unchanged.
/// Any other value is rejected by equality.
///
/// Example - accept one exact value:
/// ```typst
/// #import "@tola/schema:0.0.0": literal, parse, try-parse
/// #assert.eq(parse("draft", literal("draft")), "draft")
/// #assert.eq(try-parse("final", literal("draft")).ok, false)
/// ```
///
/// Related: enum-of, variant
/// - value (any): the Typst value accepted by equality.
/// -> dictionary
#let literal = core.literal

/// Accept a value equal to one member of `values`, and return it unchanged.
/// Any other value is rejected, and an empty `values` array is a programming error.
///
/// Example - restrict a value to a fixed set:
/// ```typst
/// #import "@tola/schema:0.0.0": enum-of, parse, try-parse
/// #let channel = enum-of(("r", "g", "b"))
/// #assert.eq(parse("g", channel), "g")
/// #assert.eq(try-parse("x", channel).ok, false)
/// ```
///
/// Related: literal
/// - values (array): a nonempty array of accepted values.
/// -> dictionary
#let enum-of = core.enum-of

/// Parse every array member in index order and return a new array of the normalized members.
/// A failed member contributes its own issue, and the issue path starts with that member's index.
/// An empty array succeeds; use `non-empty(array-of(member))` to require at least one member. Any
/// other value kind is a type issue.
///
/// Example - normalize every array member:
/// ```typst
/// #import "@tola/schema:0.0.0": array-of, trim, parse
/// #assert.eq(parse((" a ", "b"), array-of(trim(str))), ("a", "b"))
/// ```
///
/// Example - reject an empty array with `non-empty`:
/// ```typst
/// #import "@tola/schema:0.0.0": array-of, non-empty, parse, try-parse
/// #assert.eq(parse((), array-of(str)), ())
/// #assert.eq(try-parse((), non-empty(array-of(str))).ok, false)
/// ```
///
/// Related: non-empty, one-or-many
/// - member (type | dictionary): a Typst type or schema declaration for every array member.
/// -> dictionary
#let array-of = core.array-of

/// Parse every dictionary value in insertion order, keeping each literal string key.
///
/// The result is a new dictionary.
/// A failed value contributes its own issue under that key. An empty dictionary succeeds. Use
/// `schema` when the keys have different requirements, and `dictionary-of(int)` to accept
/// arbitrary keys whose values are integers. Any other value kind is a type issue.
///
/// Example - normalize every dictionary value:
/// ```typst
/// #import "@tola/schema:0.0.0": dictionary-of, trim, parse
/// #assert.eq(parse((first: " Ada "), dictionary-of(trim(str))), (first: "Ada"))
/// ```
///
/// Example - report a failing value at its key:
/// ```typst
/// #import "@tola/schema:0.0.0": dictionary-of, parse, try-parse
/// #let counts = dictionary-of(int)
/// #assert.eq(parse((one: 1, two: 2), counts), (one: 1, two: 2))
/// #let failure = try-parse((one: 1, two: "two"), counts)
/// #assert.eq(failure.ok, false)
/// #assert.eq(failure.issues.at(0).path, ("two",))
/// ```
///
/// Related: schema
/// - member (type | dictionary): a Typst type or schema declaration for every dictionary value.
/// -> dictionary
#let dictionary-of = core.dictionary-of

/// Accept an array as multiple members, or a scalar as one member, and return an array.
/// The scalar path takes any non-array value, and its failure carries no array index.
/// `parse("tag", one-or-many(str))` returns `("tag",)`, and an empty array returns `()`.
///
/// Example - accept one member or an array of them:
/// ```typst
/// #import "@tola/schema:0.0.0": one-or-many, parse
/// #assert.eq(parse("tag", one-or-many(str)), ("tag",))
/// #assert.eq(parse(("a", "b"), one-or-many(str)), ("a", "b"))
/// ```
///
/// Related: array-of
/// - member (type | dictionary): a Typst type or schema declaration for each parsed member.
/// -> dictionary
#let one-or-many = core.one-or-many

/// Parse an array of fixed positional members and an optional homogeneous `rest`.
///
/// The result is the normalized array.
/// The fixed members set the minimum count even when their schemas are optional, and `none` for
/// `rest` fixes the count and forbids extra members. Present members are still checked when the
/// count is wrong: a `schema.arity` issue reports the count, alongside any member issues.
///
/// Example - parse fixed positions with a homogeneous rest:
/// ```typst
/// #import "@tola/schema:0.0.0": tuple, parse, try-parse
/// #let pair = tuple(str, int, rest: bool)
/// #assert.eq(parse(("x", 2, true), pair), ("x", 2, true))
/// #assert.eq(try-parse(("x",), pair).ok, false)
/// ```
///
/// Example - fix the member count without `rest`:
/// ```typst
/// #import "@tola/schema:0.0.0": parse, tuple, try-parse
/// #let pair = tuple(str, int)
/// #assert.eq(parse(("x", 2), pair), ("x", 2))
/// #assert.eq(try-parse(("x", 2, true), pair).ok, false)
/// ```
///
/// Related: array-of
/// - rest (none | type | dictionary): the schema for members beyond the fixed positions;
///   `none` forbids extra members.
/// -> dictionary
#let tuple = core.tuple

/// Defer a zero-argument schema factory until the node holding it is parsed.
/// The schema the factory returns decides the value, and it may be recursive. An invalid factory
/// return is a programming error.
///
/// Example - a tree with optional children:
/// ```typst
/// #import "@tola/schema:0.0.0": schema, optional, array-of, lazy, parse
/// #let node-schema() = schema((
///   name: str,
///   children: optional(array-of(lazy(node-schema)), default: ()),
/// ))
/// #assert.eq(parse((name: "root"), node-schema()), (name: "root", children: ()))
/// ```
///
/// Related: schema, array-of
/// - factory (function): a zero-argument schema factory; it may return a recursive schema
///   without a registry, cached default, or custom recursion limit.
/// -> dictionary
#let lazy = core.lazy

/// Accept the first branch that succeeds, including one that allows an absent key.
/// Branches are tried in the order you pass them, so that order also chooses the normalized result
/// when several branches accept the value. A missing key reports a single `is required`, dropping
/// the branches whose only reported cause is that absence; a branch that carries its own notes or
/// children keeps its report. A branch that fails for any other reason contributes issues.
/// The first successful branch's value is returned. A programming error inside a branch
/// propagates instead of failing that alternative. Use `variant` for tagged objects when only one
/// branch should run, and pass schemas positionally, for example `union(str, int)`.
///
/// Example - take the first successful branch:
/// ```typst
/// #import "@tola/schema:0.0.0": union, trim, parse
/// #assert.eq(parse(" x ", union(trim(str), str)), "x")
/// ```
///
/// Example - choose between two object shapes:
/// ```typst
/// #import "@tola/schema:0.0.0": parse, schema, union
/// #let contact = union(schema((email: str)), schema((phone: str)))
/// #assert.eq(parse((email: "ada@example.com"), contact), (email: "ada@example.com"))
/// #assert.eq(parse((phone: "555-0100"), contact), (phone: "555-0100"))
/// ```
/// Related: variant
/// -> dictionary
#let union = core.union

/// Select one object branch by its explicit tag.
/// `tag-key` names the string key that selects a branch, and every branch must declare it as a
/// distinct required `literal(...)`. Pass each branch as a `schema(...)` declaration, optionally
/// annotated with `describe`.
/// A missing or unknown tag is reported at that key, and no other branch runs. The selected
/// branch's value is returned. Checks and conversions belong inside their fields or outside the
/// complete variant.
///
/// Example - select an object shape without evaluating unrelated branches:
/// ```typst
/// #import "@tola/schema:0.0.0": variant, schema, literal, parse
/// #let block-schema = variant("kind",
///   schema((kind: literal("text"), body: str)),
///   schema((kind: literal("image"), width: int)),
/// )
/// #assert.eq(parse((kind: "text", body: "Hello"), block-schema).body, "Hello")
/// ```
///
/// Example - report a missing tag at its key:
/// ```typst
/// #import "@tola/schema:0.0.0": literal, schema, try-parse, variant
/// #let block-schema = variant("kind", schema((kind: literal("text"), body: str)))
/// #let failure = try-parse((body: "Hello"), block-schema)
/// #assert.eq(failure.ok, false)
/// #assert.eq(failure.issues.at(0).path, ("kind",))
/// ```
///
/// Related: literal, schema, union
/// - tag-key (string): the literal string key that selects a branch.
/// -> dictionary
#let variant = core.variant

/// Trim a successful string and return it.
/// Any other value kind, including `none`, fails.
///
/// Example - trim a successful string:
/// ```typst
/// #import "@tola/schema:0.0.0": trim, parse, try-parse
/// #assert.eq(parse("  padded\n", trim(str)), "padded")
/// #assert.eq(try-parse(7, trim(str)).ok, false)
/// ```
///
/// Related: non-empty
/// - member (type | dictionary): the schema whose successful string is trimmed.
/// -> dictionary
#let trim = core.trim

/// Require a successful string, array, or dictionary to be nonempty, and return it unchanged.
/// An empty string, array, or dictionary fails, and any other value kind fails too.
///
/// Order matters: `non-empty(trim(str))` rejects whitespace-only text, whereas
/// `trim(non-empty(str))` accepts that text and returns an empty string.
///
/// Example - reject an empty value:
/// ```typst
/// #import "@tola/schema:0.0.0": non-empty, try-parse
/// #assert.eq(try-parse("x", non-empty(str)).ok, true)
/// #assert.eq(try-parse("", non-empty(str)).ok, false)
/// ```
///
/// Related: trim, min-length
/// - member (type | dictionary): the schema whose normalized string, array, or dictionary
///   must be nonempty.
/// -> dictionary
#let non-empty = core.non-empty

/// Reject a successful value when the predicate returns `false`.
/// The successful value is returned unchanged. A nonboolean return or a callback panic is a
/// programming error.
/// Pass `message` explicitly as a string. Although the declaration's signature defaults it to
/// `none`, construction rejects both `none` and an omitted message.
///
/// Example - reject a value with a predicate:
/// ```typst
/// #import "@tola/schema:0.0.0": refine, parse, try-parse
/// #let even = refine(int, value => calc.rem(value, 2) == 0, message: "must be even")
/// #assert.eq(parse(4, even), 4)
/// #assert.eq(try-parse(5, even).ok, false)
/// ```
///
/// Related: check
/// - member (type | dictionary): the schema whose successful value is passed to the predicate.
/// - predicate (function): the boolean predicate applied to the successful value.
/// - message (string): the required message of the `custom` issue a `false` predicate reports.
/// -> dictionary
#let refine = core.refine

/// Inspect the successful values of declared fields and report the callback's Issues.
///
/// `on` accepts the distinct declared field keys whose successful values form the callback input;
/// omit it to inspect the whole successful value. A named field is read from the same object pass,
/// even when an unrelated field fails. A failed dependency blocks the callback, and an absent one
/// stays absent. Return `()` for success or `(issue(...), ...)` for failure; paths are relative to
/// the value being checked.
///
/// Example - validate a relationship after its two fields have parsed:
/// ```typst
/// #import "@tola/schema:0.0.0": schema, check, issue, try-parse
/// #let interval = check(schema((start: int, end: int)), bounds => {
///   if bounds.end >= bounds.start { () }
///   else { (issue("must not precede start", path: ("end",)),) }
/// }, on: ("start", "end"))
/// #assert.eq(try-parse((start: 2, end: 1), interval).ok, false)
/// ```
///
/// Related: issue, refine
/// - member (type | dictionary): the schema whose successful value is inspected.
/// - inspect (function): the callback that receives the successful value and returns an array
///   of Issues.
/// - on (none | array): the distinct declared field keys whose successful values form the
///   callback input. `on` cannot cross a conversion, union, variant, or lazy boundary.
/// -> dictionary
#let check = core.check

/// Convert the inner successful value once, then parse the result through `output`.
/// Accept a function or a callable Typst type as the converter, and `output` defaults to `any`.
/// The converted value is returned on success; an inner failure or a failure against `output` is
/// reported instead. A dictionary shaped like a Result is an ordinary value here. For example,
/// `parse(7, map(int, str, output: non-empty(str)))` returns `"7"`.
///
/// Example - convert a successful value:
/// ```typst
/// #import "@tola/schema:0.0.0": map, parse
/// #assert.eq(parse(7, map(int, str, output: str)), "7")
/// ```
///
/// Related: convert
/// - member (type | dictionary): the schema whose successful value is passed to the converter.
/// - convert-value (function | type): a function or a callable Typst type returning a plain value.
/// - output (type | dictionary): the schema the converted value is parsed through.
/// -> dictionary
#let map = core.map

/// Convert the inner successful value once with a callback that returns a Result.
///
/// The result is parsed through `output`.
/// Accept a function or a callable Typst type as the callback, and `output` defaults to `any`.
/// The converted value is returned on success. Returning `err(...)` reports a value failure, and a
/// panic in the callback is a programming error that passes through `try-parse`. Use `map` for a
/// callback that returns a plain value.
///
/// Example - reject a conversion with a structured cause:
/// ```typst
/// #import "@tola/schema:0.0.0": convert, ok, err, issue, try-parse
/// #let port = convert(int, value => {
///   if value >= 0 and value <= 65535 { ok(str(value)) }
///   else { err(issue("must be between 0 and 65535")) }
/// }, output: str)
/// #assert.eq(try-parse(443, port), (ok: true, value: "443"))
/// ```
///
/// Example - report a rejected conversion with its Issue:
/// ```typst
/// #import "@tola/schema:0.0.0": convert, err, issue, ok, try-parse
/// #let port = convert(int, value => {
///   if value >= 0 and value <= 65535 { ok(str(value)) }
///   else { err(issue("must be between 0 and 65535", code: "port.range")) }
/// }, output: str)
/// #let failure = try-parse(70000, port)
/// #assert.eq(failure.ok, false)
/// #assert.eq(failure.issues.at(0).code, "port.range")
/// ```
///
/// Related: map, err
/// - member (type | dictionary): the schema whose successful value is passed to the converter.
/// - convert-value (function | type): the callback returning `ok(value)` or `err(..issues)`.
/// - output (type | dictionary): the schema the successful converted value is parsed through.
/// -> dictionary
#let convert = core.convert

/// Attach documentation to the schema's successful value; parsing and Issues are unaffected.
/// The successful value is returned unchanged.
/// An outer description replaces an inner description at the same node. Describe a nested field
/// separately to give that field its own documentation; use `issue(..., notes: ...)` for
/// diagnostics.
///
/// Example - document a successful value:
/// ```typst
/// #import "@tola/schema:0.0.0": describe, parse
/// #let slug = describe(str, "lowercase URL segment")
/// #assert.eq(parse("post", slug), "post")
/// ```
///
/// Related: inspect, issue
/// - member (type | dictionary): the schema whose successful value the description documents.
/// - description (string): the documentation attached to the successful value.
/// -> dictionary
#let describe = core.describe

/// Describe successful output values from the declaration alone, without running any callback.
///
/// Each node of the returned tree has `kind`, `presence`, and `description`. A node for a
/// Typst type also has `type`; an object has `fields` and `unknown`; containers hold
/// their members; unions and variants retain their alternatives. A lazy factory stays
/// `kind: "unknown"`.
///
/// `presence: "required"` guarantees a successful object field is present; `"optional"` describes
/// a declared omission path, which an earlier union branch may shadow; `"unknown"` is unproved.
/// A `default: (value: ...)` carries the raw declared default, including `none`, without
/// validating or converting it. Absent default keys stay absent. Container positions are always
/// present. `map` and `convert` describe their output shape while keeping their input's presence
/// and default. Input documentation never describes generated output.
///
/// The result is a conservative output contract: it is neither the input acceptance domain nor a
/// sample from evaluation.
///
/// Example - derive an output description without running callbacks:
/// ```typst
/// #import "@tola/schema:0.0.0": schema, optional, inspect
/// #let profile = inspect(schema((title: str, subtitle: optional(str))))
/// #assert.eq(profile.fields.title.presence, "required")
/// #assert.eq(profile.fields.subtitle.presence, "optional")
/// ```
///
/// Related: describe, map, convert
/// - member (type | dictionary): the schema whose output description is derived.
/// -> dictionary
#let inspect = core.inspect

/// Parse once and return the complete normalized value, or fail with `format-issues` output.
///
/// Example - return a normalized value in one call:
/// ```typst
/// #import "@tola/schema:0.0.0": non-empty, trim, parse
/// #assert.eq(parse("  Notes  ", non-empty(trim(str))), "Notes")
/// ```
///
/// Related: try-parse, format-issues
/// - value (any): the value to parse.
/// - member (type | dictionary): the schema the value is parsed through.
/// -> any
#let parse = core.parse

/// Try one parse and return exactly `(ok: true, value: value)` or `(ok: false, issues: issues)`.
/// Failure issues are nonempty and never accompany a partial value.
/// Schema validation failures come back as a Result; malformed declarations, invalid callback
/// return shapes, and callback panics still stop compilation. The input is always present,
/// including `none`. Missing keys arise only while parsing an object.
///
/// Example - return a Result instead of failing:
/// ```typst
/// #import "@tola/schema:0.0.0": try-parse
/// #assert.eq(try-parse(7, int), (ok: true, value: 7))
/// #assert.eq(try-parse("7", int).ok, false)
/// ```
///
/// Related: parse, format-issues
/// - value (any): the value to parse.
/// - member (type | dictionary): the schema the value is parsed through.
/// -> dictionary
#let try-parse = core.try-parse

/// Construct a successful Result from any value.
/// `ok(none)` is a success whose value is `none`.
///
/// Example - construct a successful Result:
/// ```typst
/// #import "@tola/schema:0.0.0": ok
/// #assert.eq(ok(none), (ok: true, value: none))
/// ```
///
/// Related: err
/// - value (any): the value the successful Result carries.
/// -> dictionary
#let ok = core.ok

/// Construct a failed Result from one or more Issues.
/// `err()` with no Issue is a programming error. The Result carries `ok: false` and the Issues.
///
/// Example - construct a failed Result from Issues:
/// ```typst
/// #import "@tola/schema:0.0.0": err, issue
/// #let failure = err(issue("too small", code: "size", path: ("count",)))
/// #assert.eq(failure.ok, false)
/// #assert.eq(failure.issues.at(0).code, "size")
/// ```
/// Related: ok, issue
/// -> dictionary
#let err = core.err

/// Construct one Issue: a single problem a schema reports.
///
/// A path is relative to the checked value: `("author.name",)` names one key, `("author", "name")`
/// names a nested key, and `("authors", 0)` names an array member.
/// To add nested causes, spread the Issue into a dictionary and replace `children` with an array
/// of Issues.
///
/// Example - build an Issue with a path:
/// ```typst
/// #import "@tola/schema:0.0.0": issue
/// #let problem = issue("duplicate tag", path: ("tags", 0), notes: ("remove the repeat",))
/// #assert.eq(problem.path, ("tags", 0))
/// ```
///
/// Related: format-issues, err
/// - message (string): the human-readable problem description.
/// - code (string): the machine-readable problem code.
/// - path (array): literal string keys or nonnegative indices relative to the checked value.
/// - notes (array): an array of string notes, empty by default.
/// -> dictionary
#let issue = core.issue

/// Render Issue trees to deterministic text with unambiguous paths, notes, and branch causes.
/// The text names whether a failing value came from a declared default or a conversion, using the
/// Issue data alone. Each issue becomes one line, and its child causes are indented below it. A
/// malformed Issue stops compilation.
///
/// Example - render an Issue tree as text:
/// ```typst
/// #import "@tola/schema:0.0.0": issue, format-issues
/// #let problem = issue("is required", code: "schema.missing", path: ("title",))
/// #assert.eq(format-issues((problem,)), "`$.title`: is required")
/// ```
///
/// Example - render nested causes and notes:
/// ```typst
/// #import "@tola/schema:0.0.0": format-issues, issue
/// #let cause = issue("is required", code: "schema.missing", path: ("title",), notes: ("add a title",))
/// #let tree = (..issue("metadata is invalid", code: "metadata.invalid"), children: (cause,))
/// #assert.eq(format-issues((tree,)), "`$`: metadata is invalid\n  `$.title`: is required\n    note: add a title")
/// ```
///
/// Related: issue, parse
/// - issues (array): the Issue values to render.
/// -> string
#let format-issues = core.format-issues

/// Require at least `minimum` grapheme clusters, array members, or dictionary entries.
///
/// A shorter value is rejected. Any other value kind is a type issue. The measured value is
/// returned unchanged.
///
/// Example - require a minimum length:
/// ```typst
/// #import "@tola/schema:0.0.0": min-length, try-parse
/// #assert.eq(try-parse("ab", min-length(str, 3)).ok, false)
/// #assert.eq(try-parse("abc", min-length(str, 3)).ok, true)
/// ```
///
/// Related: max-length
/// - member (type | dictionary): the schema whose successful value is measured.
/// - minimum (int): the nonnegative minimum number of clusters, members, or entries.
/// -> dictionary
#let min-length = core.min-length

/// Allow at most `maximum` grapheme clusters, array members, or dictionary entries.
///
/// A longer value is rejected. Any other value kind is a type issue. The measured value is
/// returned unchanged.
///
/// Example - cap the length of a value:
/// ```typst
/// #import "@tola/schema:0.0.0": max-length, try-parse
/// #assert.eq(try-parse((1, 2), max-length(array, 2)).ok, true)
/// #assert.eq(try-parse((1, 2, 3), max-length(array, 2)).ok, false)
/// ```
///
/// Related: min-length
/// - member (type | dictionary): the schema whose successful value is measured.
/// - maximum (int): the nonnegative maximum number of clusters, members, or entries.
/// -> dictionary
#let max-length = core.max-length

/// Require a string to match a Typst regex value; a string pattern is rejected.
/// Matching searches anywhere in the string, so add anchors for a whole-string constraint. The
/// regex sees the string exactly as parsed, and the accepted string is returned unchanged.
///
/// Example - require a regex match:
/// ```typst
/// #import "@tola/schema:0.0.0": matches, try-parse
/// #let slug = matches(str, regex("^[a-z0-9-]+$"))
/// #assert.eq(try-parse("my-post", slug).ok, true)
/// #assert.eq(try-parse("My Post", slug).ok, false)
/// ```
///
/// Related: trim
/// - member (type | dictionary): the schema whose successful string is matched.
/// - expression (regex): the regular expression searched in the string.
/// -> dictionary
#let matches = core.matches

/// Require an integer or float greater than or equal to `minimum`.
/// The accepted value is returned unchanged. NaN fails; infinities compare like any other number.
///
/// Example - enforce an inclusive lower bound:
/// ```typst
/// #import "@tola/schema:0.0.0": at-least, try-parse
/// #assert.eq(try-parse(5, at-least(int, 5)).ok, true)
/// #assert.eq(try-parse(4, at-least(int, 5)).ok, false)
/// ```
///
/// Related: at-most
/// - member (type | dictionary): the schema whose successful value is compared.
/// - minimum (int | float): the bound, an integer or float other than NaN.
/// -> dictionary
#let at-least = core.at-least

/// Require an integer or float less than or equal to `maximum`.
/// The accepted value is returned unchanged. NaN fails; infinities compare like any other number.
///
/// Example - enforce an inclusive upper bound:
/// ```typst
/// #import "@tola/schema:0.0.0": at-most, try-parse
/// #assert.eq(try-parse(5, at-most(int, 5)).ok, true)
/// #assert.eq(try-parse(6, at-most(int, 5)).ok, false)
/// ```
///
/// Related: at-least
/// - member (type | dictionary): the schema whose successful value is compared.
/// - maximum (int | float): the bound, an integer or float other than NaN.
/// -> dictionary
#let at-most = core.at-most

/// An unquoted ASCII dot-atom mailbox per RFC 5322.
/// The local part is nonempty dot-separated atext atoms of at most 64 bytes, followed by `@` and
/// one or more DNS-style labels. Each label holds 1-63 ASCII letters, digits, or hyphens and
/// starts and ends with a letter or digit; the whole address is at most 254 bytes, and a
/// single-label domain is accepted.
/// The grammar covers the bare address only: quotes, comments, display names, domain literals,
/// and Unicode lie outside it. The check is syntactic: no trimming, DNS lookup, or deliverability
/// test.
///
/// Example - validate an email address:
/// ```typst
/// #import "@tola/schema:0.0.0": email, try-parse
/// #assert.eq(try-parse("ada@example.com", email).ok, true)
/// #assert.eq(try-parse("not an address", email).ok, false)
/// ```
/// Related: matches
#let email = core.email

/// An IPv4 address in dotted decimal: four octets in 0-255, without redundant leading zeros.
/// The accepted spelling is returned unchanged. Brackets, a port, a zone identifier, and
/// surrounding whitespace are rejected.
///
/// Example - validate an IPv4 address:
/// ```typst
/// #import "@tola/schema:0.0.0": ipv4, try-parse
/// #assert.eq(try-parse("192.168.0.1", ipv4).ok, true)
/// #assert.eq(try-parse("192.168.0.256", ipv4).ok, false)
/// ```
/// Related: ip
#let ipv4 = core.ipv4

/// An IPv6 address.
///
/// Eight 1-4-digit hexadecimal groups are accepted, or one `::` replacing at least one group.
/// A strict dotted-decimal IPv4 suffix may replace the final two groups. The spelling is returned
/// unchanged. Brackets, ports, zone identifiers, and surrounding whitespace are rejected.
///
/// Example - validate an IPv6 address:
/// ```typst
/// #import "@tola/schema:0.0.0": ipv6, try-parse
/// #assert.eq(try-parse("2001:db8::1", ipv6).ok, true)
/// #assert.eq(try-parse("2001:db8::1::2", ipv6).ok, false)
/// ```
/// Related: ip
#let ipv6 = core.ipv6

/// An IPv4 or IPv6 address, returned unchanged.
/// Brackets, a zone identifier, and a port are rejected.
///
/// Example - validate either IP family:
/// ```typst
/// #import "@tola/schema:0.0.0": ip, try-parse
/// #assert.eq(try-parse("::1", ip).ok, true)
/// #assert.eq(try-parse("127.0.0.1:80", ip).ok, false)
/// ```
/// Related: ipv4, ipv6
#let ip = core.ip

/// An absolute HTTP(S) URI/IRI with a case-insensitive scheme and a nonempty authority/host.
/// RFC 3986 components are supported: userinfo, registered names, bracketed IPv6/IPvFuture,
/// valid percent escapes, and an empty port or a decimal port in 0-65535. Unicode uses RFC 3987
/// ucschar, and private-use characters are allowed only in the query. The accepted spelling is
/// returned unchanged. Whitespace and control characters are rejected, and the check is
/// syntactic: no trimming, percent decoding, normalization, or DNS lookup.
///
/// Example - validate an absolute HTTP(S) URL:
/// ```typst
/// #import "@tola/schema:0.0.0": http-url, try-parse
/// #assert.eq(try-parse("https://example.com/posts", http-url).ok, true)
/// #assert.eq(try-parse("example.com/posts", http-url).ok, false)
/// ```
/// Related: https-url
#let http-url = core.http-url

/// An absolute HTTPS URI/IRI in the same syntax as `http-url`.
///
/// The accepted spelling is returned unchanged, without a network or deliverability check.
///
/// Example - restrict a URL to HTTPS:
/// ```typst
/// #import "@tola/schema:0.0.0": https-url, try-parse
/// #assert.eq(try-parse("https://example.com", https-url).ok, true)
/// #assert.eq(try-parse("http://example.com", https-url).ok, false)
/// ```
/// Related: http-url
#let https-url = core.https-url

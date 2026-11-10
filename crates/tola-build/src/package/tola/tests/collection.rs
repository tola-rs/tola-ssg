use super::{assert_package, reject_package};

fn assert_collection(assertions: &str) {
    assert_package("collection", assertions);
}

fn reject_collection(assertions: &str) {
    reject_package("collection", assertions);
}

#[test]
fn membership_shapes_are_not_coerced() {
    for expression in [
        "group-by(((tags: (\"rust\",)),), key: member => member.tags)",
        "group-by-keys(((tag: \"rust\"),), keys: member => member.tag)",
        "group-by-keys((none,))",
        "select-members(((tags: none),), (\"rust\",), keys: member => member.tags)",
        "select-members(((\"rust\",),), \"rust\")",
        "pick((name: \"Author\"), (1,))",
    ] {
        reject_collection(&format!("#let selected = {expression}"));
    }
}

#[test]
fn sparse_parents_remain_traversable() {
    assert_collection(
        r#"
#let members = ("leaf", "root")
#let parent(key) = (leaf: "missing", missing: "root", root: none).at(key)
#assert.eq(ancestors(members, "leaf", parent), ("root",))
#assert.eq(descendants(members, "root", parent), ("leaf",))
#assert.eq(children(members, "missing", parent), ("leaf",))
#assert.eq(descendants(members, "missing", parent), ("leaf",))
"#,
    );
}

#[test]
fn hierarchy_accepts_composite_identities() {
    assert_collection(
        r#"
#let root = (id: ("root", 1))
#let leaf = (id: ("leaf", 2))
#let parent(key) = if key == leaf.id { root.id } else { none }
#assert.eq(ancestors((leaf, root), leaf.id, parent, key: member => member.id), (root,))
#assert.eq(children((leaf, root), root.id, parent, key: member => member.id), (leaf,))
"#,
    );
}

#[test]
fn duplicate_navigation_keys_are_errors() {
    for expression in [
        "index-by(members)",
        "adjacent(members, \"anchor\")",
        "window(members, \"anchor\", before: 1)",
        "children(members, \"root\", _ => none)",
        "descendants(members, \"root\", _ => none)",
        "ancestors(members, \"anchor\", _ => none)",
        "lineage(members, \"anchor\", _ => none)",
        "siblings(members, \"anchor\", _ => none)",
    ] {
        reject_collection(&format!(
            "#let members = (\"anchor\", \"duplicate\", \"duplicate\")\n\
             #let selected = {expression}"
        ));
    }
}

#[test]
fn traversed_parent_cycles_are_errors() {
    for operation in ["ancestors", "lineage", "descendants"] {
        for parents in [
            r#"(leaf: "leaf")"#,
            r#"(leaf: "missing", missing: "leaf")"#,
            r#"(leaf: "root", root: "leaf")"#,
        ] {
            let anchor = if operation == "descendants" {
                "root"
            } else {
                "leaf"
            };
            reject_collection(&format!(
                "#let parent(key) = {parents}.at(key)\n\
                 #let selected = {operation}((\"leaf\",), \"{anchor}\", parent)"
            ));
        }
    }
}

#[test]
fn direct_relations_ignore_deeper_cycles() {
    assert_collection(
        r#"
#let members = ("leaf-a", "leaf-b", "root")
#let parent(key) = ("leaf-a": "root", "leaf-b": "root", root: "missing", missing: "root").at(key)
#assert.eq(children(members, "root", parent), ("leaf-a", "leaf-b"))
#assert.eq(siblings(members, "leaf-a", parent), ("leaf-b",))
"#,
    );
}

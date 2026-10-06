//! Every diagnostic identifier this crate's own producers declare.

/// Editor integration.
pub mod editor {
    use tola_build::diagnostic::DiagnosticCode;

    /// The site configuration an editor check needs could not be loaded.
    pub const CONFIGURATION: DiagnosticCode = DiagnosticCode::new("editor.configuration");

    /// A `break` or `continue` no loop encloses.
    pub const BRANCH_OUTSIDE_LOOP: DiagnosticCode =
        DiagnosticCode::new("editor.branch_outside_loop");

    /// A `return` no function encloses.
    pub const RETURN_OUTSIDE_FUNCTION: DiagnosticCode =
        DiagnosticCode::new("editor.return_outside_function");

    /// A font family this environment does not hold.
    pub const UNKNOWN_FONT: DiagnosticCode = DiagnosticCode::new("editor.unknown_font");

    /// A math spelling no scope, import, or the math library defines.
    pub const UNKNOWN_MATH_VARIABLE: DiagnosticCode =
        DiagnosticCode::new("editor.unknown_math_variable");

    /// A `set` or `show` statement in a block that produces no content for it to affect.
    pub const INEFFECTIVE_SET_SHOW: DiagnosticCode =
        DiagnosticCode::new("editor.ineffective_set_show");

    /// A value evaluated before an explicit `return` that discards it.
    pub const DISCARDED_BY_RETURN: DiagnosticCode =
        DiagnosticCode::new("editor.discarded_by_return");

    /// A `let` or `for` binding no reachable read reaches.
    pub const UNUSED_BINDING: DiagnosticCode = DiagnosticCode::new("editor.unused_binding");

    /// A stored value no reachable read observes.
    pub const DEAD_STORE: DiagnosticCode = DiagnosticCode::new("editor.dead_store");

    /// An import from a `@tola/…` package the site cannot resolve.
    pub const UNRESOLVED_PACKAGE_IMPORT: DiagnosticCode =
        DiagnosticCode::new("editor.unresolved_package_import");
}

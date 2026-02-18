//! Template types for typed variable injection.

use std::marker::PhantomData;

/// Trait for template variable sets
pub trait TemplateVars {
    fn apply(&self, content: &str) -> String;

    /// Returns a string representation of the variables for hash computation.
    /// Override this to provide a more efficient hash input than the full rendered content.
    fn hash_input(&self) -> String {
        String::new()
    }
}

/// Template with typed variable injection
#[derive(Debug, Clone, Copy)]
pub struct Template<V> {
    content: &'static str,
    _marker: PhantomData<V>,
}

impl<V> Template<V> {
    pub const fn new(content: &'static str) -> Self {
        Self {
            content,
            _marker: PhantomData,
        }
    }

    #[allow(dead_code)]
    pub const fn content(&self) -> &'static str {
        self.content
    }
}

impl<V: TemplateVars> Template<V> {
    pub fn render(&self, vars: &V) -> String {
        vars.apply(self.content)
    }
}

//! Tola site modules.

pub mod action;
mod archive;
pub mod arguments;

pub mod assets;
pub mod check;
pub mod checkout;
pub mod declaration;
pub mod entities;
pub mod hooks;
pub mod inputs;
pub mod manifest;
pub mod module_set;
pub mod options;
pub mod pack;
pub mod prepared;
pub mod query;
pub mod remote;
pub mod watch;

pub use action::Action;
pub use arguments::expand_argument;
pub use checkout::CheckoutError;
pub use declaration::*;
pub use hooks::{
    BeforePackHook, CommandOutput, DevParticipation, HookDeclarationError, HookStage,
    ModuleHookOrigin, ModuleHooks,
};
pub use manifest::{ModuleDeclarations, NuModule};
pub use module_set::{
    LoadedModule, ManifestEvidence, ModuleCommands, ModuleLoadError, ModulePathError,
    load_modules_with_roots,
};
pub use prepared::{
    Acquired, AcquiredModule, AssetContribution, Context as PrepareContext, Contribution,
    EntrySelection, HookEntry, HooksContribution, ModuleEntry, PreparedModules, acquire, prepare,
    select_query, select_query_file,
};
pub use remote::FetchError;
pub use watch::HookWatchInputs;

//! Tola site modules.

pub mod action;
mod archive;
pub mod arguments;

pub mod assets;
mod builtin;
pub mod bundle;
pub mod check;
pub mod checkout;
pub mod declaration;
mod diagnostic;
pub mod entities;
pub mod fragment;
pub mod hooks;
pub mod icons;
pub mod identity;
pub mod inputs;
pub mod manifest;
pub mod module_set;
mod option_refs;
pub mod options;
pub mod pack;
pub mod query;
pub mod remote;
pub mod watch;

pub use action::Action;
pub use arguments::expand_argument;
pub use builtin::{BuiltinModule, install_builtin_modules};
pub use bundle::{
    Acquired, AcquiredModule, AssetContribution, Context as PrepareContext, Contribution,
    EntrySelection, HookEntry, HooksContribution, ModuleBundle, ModuleEntry, acquire,
    acquire_with_lock, prepare, select_query, select_query_file,
};
pub use checkout::CheckoutError;
pub use declaration::*;
pub use diagnostic::error_diagnostic;
pub use hooks::{
    BeforePackHook, CommandOutput, DevParticipation, HookDeclarationError, HookStage,
    ModuleHookOrigin, ModuleHooks,
};
pub use identity::{InvalidModuleName, ModuleAlias, ModuleCapability, ModuleId, TolaVersion};
pub use inputs::vendor_module_dir;
pub use manifest::{ModuleDeclarations, NuModule};
pub use module_set::{
    LoadedModule, ManifestStamp, ModuleCommands, ModuleLoadError, ModuleOwner, ModulePathError,
    load_modules_with_roots,
};
pub use remote::FetchError;
pub use watch::HookWatchInputs;

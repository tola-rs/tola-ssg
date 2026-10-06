//! The wire one connection speaks: the requests it decodes, the replies it sends, and the
//! settings a client negotiates.

use crate::routes::RouteIndexReply;
use lsp_server::{ErrorCode, Request, RequestId, Response};
use lsp_types::request::{self, Request as LspRequest};
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    CallHierarchyServerCapability, ChangeAnnotation, CodeAction, CodeActionKind, CodeActionOptions,
    CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CodeLens, CodeLensOptions,
    CodeLensParams, ColorInformation, ColorPresentation, ColorPresentationParams,
    ColorProviderCapability, CompletionOptions, CompletionResponse, CreateFile, CreateFileOptions,
    DiagnosticOptions, DiagnosticServerCapabilities, DocumentChangeOperation, DocumentChanges,
    DocumentColorParams, DocumentDiagnosticParams, DocumentFormattingParams, DocumentHighlight,
    DocumentHighlightParams, DocumentLinkOptions, DocumentLinkParams,
    DocumentRangeFormattingParams, DocumentSymbolParams, ExecuteCommandParams, FileOperationFilter,
    FileOperationPattern, FileOperationRegistrationOptions, FoldingRangeParams,
    FoldingRangeProviderCapability, GotoDefinitionResponse, Hover, HoverContents,
    HoverProviderCapability, InitializeParams, InitializeResult, InlayHint, InlayHintParams,
    Location, MarkupContent, MarkupKind, OneOf, Position, PositionEncodingKind,
    PrepareRenameResponse, ReferenceParams, RenameFilesParams, RenameOptions, RenameParams,
    ResourceOp, SaveOptions, SelectionRangeParams, SelectionRangeProviderCapability,
    SemanticTokensDeltaParams, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensParams, SemanticTokensRangeParams, SemanticTokensServerCapabilities,
    ServerCapabilities, ServerInfo, SignatureHelp, SignatureHelpOptions,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextEdit, Uri, WorkDoneProgressOptions, WorkspaceEdit,
    WorkspaceFileOperationsServerCapabilities, WorkspaceFoldersServerCapabilities,
    WorkspaceServerCapabilities, WorkspaceSymbolOptions, WorkspaceSymbolParams,
    WorkspaceSymbolResponse,
};
use serde::{Deserialize, Serialize};

pub(super) enum ClientRequest {
    Initialize(Box<InitializeParams>),
    Shutdown,
    Formatting(DocumentFormattingParams),
    RangeFormatting(DocumentRangeFormattingParams),
    Folding(FoldingRangeParams),
    Symbols(DocumentSymbolParams),
    Links(DocumentLinkParams),
    Selection(SelectionRangeParams),
    Tokens(SemanticTokensParams),
    Route(RouteParams),
    CodeLenses(CodeLensParams),
    Rename(RenameFilesParams),
    Enter(EnterParams),
    TokensDelta(SemanticTokensDeltaParams),
    TokensRange(SemanticTokensRangeParams),
    WorkspaceSymbols(WorkspaceSymbolParams),
    Query(SourceQuery),
    /// A command the client asks the server to run, which only the server can run: a lens's own
    /// command is dead in every client that has no extension behind it.
    ExecuteCommand(ExecuteCommandParams),
    Diagnostic(DocumentDiagnosticParams),
    PackageSource(PackageSourceParams),
    PrepareCallHierarchy(CallHierarchyPrepareParams),
    IncomingCalls(CallHierarchyIncomingCallsParams),
    OutgoingCalls(CallHierarchyOutgoingCallsParams),
    RouteIndex,
}

impl ClientRequest {
    pub(super) fn decode(request: Request) -> Result<(RequestId, Self), Box<Response>> {
        match request.method.as_str() {
            request::Initialize::METHOD => {
                let mut request = request;
                // lsp-types 0.97 omits the wire rename for workspace.diagnostics.
                if let Some(workspace) = request
                    .params
                    .pointer_mut("/capabilities/workspace")
                    .and_then(serde_json::Value::as_object_mut)
                    && let Some(diagnostics) = workspace.remove("diagnostics")
                {
                    workspace.insert("diagnostic".into(), diagnostics);
                }
                extract::<request::Initialize>(request)
                    .map(|(id, parameters)| (id, Self::Initialize(Box::new(parameters))))
            }
            request::Shutdown::METHOD => {
                extract::<request::Shutdown>(request).map(|(id, ())| (id, Self::Shutdown))
            }
            request::Formatting::METHOD => extract::<request::Formatting>(request)
                .map(|(id, parameters)| (id, Self::Formatting(parameters))),
            request::RangeFormatting::METHOD => extract::<request::RangeFormatting>(request)
                .map(|(id, parameters)| (id, Self::RangeFormatting(parameters))),
            request::FoldingRangeRequest::METHOD => {
                extract::<request::FoldingRangeRequest>(request)
                    .map(|(id, parameters)| (id, Self::Folding(parameters)))
            }
            request::DocumentSymbolRequest::METHOD => {
                extract::<request::DocumentSymbolRequest>(request)
                    .map(|(id, parameters)| (id, Self::Symbols(parameters)))
            }
            request::SelectionRangeRequest::METHOD => {
                extract::<request::SelectionRangeRequest>(request)
                    .map(|(id, params)| (id, Self::Selection(params)))
            }
            request::DocumentLinkRequest::METHOD => {
                extract::<request::DocumentLinkRequest>(request)
                    .map(|(id, parameters)| (id, Self::Links(parameters)))
            }
            request::SemanticTokensFullRequest::METHOD => {
                extract::<request::SemanticTokensFullRequest>(request)
                    .map(|(id, parameters)| (id, Self::Tokens(parameters)))
            }
            request::SemanticTokensFullDeltaRequest::METHOD => {
                extract::<request::SemanticTokensFullDeltaRequest>(request)
                    .map(|(id, parameters)| (id, Self::TokensDelta(parameters)))
            }
            request::SemanticTokensRangeRequest::METHOD => {
                extract::<request::SemanticTokensRangeRequest>(request)
                    .map(|(id, parameters)| (id, Self::TokensRange(parameters)))
            }
            EnterRequest::METHOD => extract::<EnterRequest>(request)
                .map(|(id, parameters)| (id, Self::Enter(parameters))),
            request::WorkspaceSymbolRequest::METHOD => {
                extract::<request::WorkspaceSymbolRequest>(request)
                    .map(|(id, parameters)| (id, Self::WorkspaceSymbols(parameters)))
            }
            request::WillRenameFiles::METHOD => extract::<request::WillRenameFiles>(request)
                .map(|(id, parameters)| (id, Self::Rename(parameters))),
            request::DocumentDiagnosticRequest::METHOD => {
                extract::<request::DocumentDiagnosticRequest>(request)
                    .map(|(id, parameters)| (id, Self::Diagnostic(parameters)))
            }
            RouteRequest::METHOD => extract::<RouteRequest>(request)
                .map(|(id, parameters)| (id, Self::Route(parameters))),
            request::CodeLensRequest::METHOD => extract::<request::CodeLensRequest>(request)
                .map(|(id, parameters)| (id, Self::CodeLenses(parameters))),
            request::ExecuteCommand::METHOD => extract::<request::ExecuteCommand>(request)
                .map(|(id, parameters)| (id, Self::ExecuteCommand(parameters))),
            crate::routes::RouteIndexRequest::METHOD => {
                extract::<crate::routes::RouteIndexRequest>(request)
                    .map(|(id, _)| (id, Self::RouteIndex))
            }
            PackageSourceRequest::METHOD => extract::<PackageSourceRequest>(request)
                .map(|(id, parameters)| (id, Self::PackageSource(parameters))),
            request::CallHierarchyPrepare::METHOD => {
                extract::<request::CallHierarchyPrepare>(request)
                    .map(|(id, parameters)| (id, Self::PrepareCallHierarchy(parameters)))
            }
            request::CallHierarchyIncomingCalls::METHOD => {
                extract::<request::CallHierarchyIncomingCalls>(request)
                    .map(|(id, parameters)| (id, Self::IncomingCalls(parameters)))
            }
            request::CallHierarchyOutgoingCalls::METHOD => {
                extract::<request::CallHierarchyOutgoingCalls>(request)
                    .map(|(id, parameters)| (id, Self::OutgoingCalls(parameters)))
            }
            _ => SourceQuery::decode(request).map(|(id, query)| (id, Self::Query(query))),
        }
    }
}

pub(super) fn initialize_result(
    features: &crate::capabilities::ClientFeatures,
) -> InitializeResult {
    InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: Some(PositionEncodingKind::UTF16),
            text_document_sync: Some(TextDocumentSyncCapability::Options(
                TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(TextDocumentSyncKind::INCREMENTAL),
                    save: Some(
                        SaveOptions {
                            include_text: Some(false),
                        }
                        .into(),
                    ),
                    ..Default::default()
                },
            )),
            completion_provider: Some(CompletionOptions {
                trigger_characters: Some(
                    ["#", "(", "<", ",", ".", ":", "/", "\"", "@"]
                        .map(str::to_owned)
                        .into(),
                ),
                resolve_provider: Some(false),
                ..Default::default()
            }),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            definition_provider: Some(OneOf::Left(true)),
            references_provider: Some(OneOf::Left(true)),
            call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
            workspace_symbol_provider: Some(OneOf::Right(WorkspaceSymbolOptions {
                resolve_provider: Some(false),
                work_done_progress_options: WorkDoneProgressOptions::default(),
            })),
            inlay_hint_provider: Some(OneOf::Left(true)),
            selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
            rename_provider: Some(OneOf::Right(RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: WorkDoneProgressOptions::default(),
            })),
            signature_help_provider: Some(SignatureHelpOptions {
                trigger_characters: Some(["(", ","].map(str::to_owned).into()),
                ..Default::default()
            }),
            document_formatting_provider: Some(OneOf::Left(true)),
            document_range_formatting_provider: Some(OneOf::Left(true)),
            document_symbol_provider: Some(OneOf::Left(true)),
            folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
            // Every action Tola offers is computed from the request itself, so a client that
            // filters by kind still finds them and none is resolved lazily. One authority names
            // the advertised kinds — the lane's own set, each once and in a fixed order — so the
            // advertisement cannot drift from what a request can select.
            code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(crate::code_actions::SERVED_KINDS.to_vec()),

                work_done_progress_options: WorkDoneProgressOptions::default(),
                resolve_provider: Some(false),
            })),
            code_lens_provider: Some(CodeLensOptions {
                resolve_provider: Some(false),
            }),
            // No `execute_command_provider` is advertised: the preview lens names a command the
            // client's own extension registers, and a client that registers every command a server
            // offers would collide with the one it already holds; the lens still opens the page.
            color_provider: Some(ColorProviderCapability::Simple(true)),
            document_highlight_provider: Some(OneOf::Left(true)),
            document_link_provider: Some(DocumentLinkOptions {
                resolve_provider: Some(false),
                work_done_progress_options: WorkDoneProgressOptions::default(),
            }),
            semantic_tokens_provider: Some(
                SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                    legend: crate::tokens::legend(),
                    full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
                    range: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
            ),
            workspace: Some(WorkspaceServerCapabilities {
                file_operations: features.will_rename_files.then(|| {
                    WorkspaceFileOperationsServerCapabilities {
                        will_rename: Some(FileOperationRegistrationOptions {
                            // A folder rename moves every site file below it, and any site file
                            // may be a link target, so the whole workspace is what the request
                            // asks about; the site's own file list decides which renames matter.
                            filters: vec![FileOperationFilter {
                                scheme: Some("file".to_owned()),
                                pattern: FileOperationPattern {
                                    glob: "**/*".to_owned(),
                                    matches: None,
                                    options: None,
                                },
                            }],
                        }),
                        ..WorkspaceFileOperationsServerCapabilities::default()
                    }
                }),
                workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                    supported: Some(false),
                    change_notifications: Some(OneOf::Left(false)),
                }),
            }),
            // A client that pulls asks for the site's diagnostics itself; one that only takes what
            // is pushed has no use for the provider, so it is offered only to the former.
            diagnostic_provider: features.pull_diagnostics.then(|| {
                DiagnosticServerCapabilities::Options(DiagnosticOptions {
                    identifier: Some("tola".to_owned()),
                    inter_file_dependencies: false,
                    workspace_diagnostics: false,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })
            }),
            // The server's own requests have no standard field, so the ones a client may call are
            // named here, each with the method it answers to and what its reply has.
            experimental: Some(serde_json::json!({
                "requests": {
                    (crate::routes::RouteIndexRequest::METHOD):
                        "every page the checked site realizes, with the source writing it and the address it answers at",
                },
            })),
            ..Default::default()
        },
        server_info: Some(ServerInfo {
            name: "tola-lsp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    }
}

/// Package-source access and status negotiation are fixed for one connection.
/// Formatting, check mode and preview origin may change through configuration notifications.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InitializationOptions {
    pub package_source_directory: Option<std::path::PathBuf>,
    #[serde(default)]
    pub formatter: FormatterOptions,
    /// When a source change reruns the site's check.
    #[serde(default)]
    pub check_mode: CheckMode,
    /// The origin the development server serves this site at, as `http://host:port` with no
    /// path: the site's own mount path is read from its configuration.
    pub preview_origin: Option<String>,
    #[serde(default)]
    pub source_check_status: bool,
}

/// When a source change reruns the site's check.
///
/// The two names are Tinymist's `OnType` and `OnSave` (Apache-2.0; see `licenses/README.md`),
/// which its `TaskWhen` marks the same way: a check spends the whole site's work whatever
/// changed, so an author on a large site trades freshness for quiet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CheckMode {
    /// Check once the author pauses typing.
    #[default]
    OnType,
    /// Check when the author saves.
    OnSave,
}

impl CheckMode {
    /// The mode one settings value names, or `None` when it names no mode.
    ///
    /// The name read here is the one this type reads its own settings by, so a mode is spelled
    /// once.
    pub(super) fn named(value: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(value.to_owned())).ok()
    }
}

/// The settings every client names the same way, read from whichever shape supplied them.
///
/// One key that is absent leaves the setting it names as it stands, so a client that changes one
/// setting does not reset the others.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Settings {
    pub formatter: FormatterSettings,
    pub check_mode: Option<CheckMode>,
    pub preview_origin: Option<String>,
}

/// The formatter settings a client named, each absent when it named none.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct FormatterSettings {
    pub print_width: Option<usize>,
    pub prose_wrap: Option<bool>,
}

impl Settings {
    /// The settings one settings value has, which is the table a client sends in
    /// `workspace/didChangeConfiguration` or one section of a `workspace/configuration` answer.
    pub(super) fn read(value: &serde_json::Value) -> Self {
        let formatter = value
            .get("formatter")
            .and_then(serde_json::Value::as_object);
        Self {
            formatter: FormatterSettings {
                print_width: formatter
                    .and_then(|formatter| formatter.get("printWidth"))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|width| usize::try_from(width).ok()),
                prose_wrap: formatter
                    .and_then(|formatter| formatter.get("proseWrap"))
                    .and_then(serde_json::Value::as_bool),
            },
            check_mode: value
                .get("checkMode")
                .and_then(serde_json::Value::as_str)
                .and_then(CheckMode::named),
            preview_origin: value
                .get("previewOrigin")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        }
    }

    /// Whether these settings named nothing this server reads.
    pub(super) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// How far the check has got when a query has nothing compiled to answer from.
///
/// The connection is the only observer that can tell these apart: its compiler lane runs one job
/// at a time, so a check that is queued or already running is invisible from inside an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CheckProgress {
    /// No check has run for this source revision and none is queued: a workspace checked on save,
    /// before the author has saved.
    NotChecked,
    /// A check for this source revision is queued or running.
    Checking,
    /// A check for this source revision finished: the site's root Bundle compiled in site mode, or
    /// at least one open document's own check resolved a world in a workspace that holds no site.
    Checked,
    /// A check for this source revision finished: the root Bundle did not compile in site mode, or
    /// no open document's own check resolved a world in a workspace that holds no site.
    Failed,
}

impl CheckProgress {
    /// The sentence an answer has when the check has nothing to answer from, true of the state
    /// it is answered in.
    pub(super) fn absent(self) -> &'static str {
        match self {
            Self::NotChecked => "the check has not run yet; save the source to run it",
            Self::Checking => "the check is still running; ask again when it finishes",
            Self::Checked => {
                "Tola could not determine this expression's value; check where it is defined"
            }
            Self::Failed => "the sources could not be compiled; fix the reported errors",
        }
    }
}

/// How the client's own settings shape the formatter's output.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct FormatterOptions {
    pub print_width: usize,
    pub prose_wrap: bool,
}

impl Default for FormatterOptions {
    fn default() -> Self {
        Self {
            print_width: DEFAULT_PRINT_WIDTH,
            prose_wrap: false,
        }
    }
}

/// The width a source is formatted to when the client declares none.
const DEFAULT_PRINT_WIDTH: usize = 120;

/// The position one Enter key press acts on.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct EnterParams {
    pub uri: Uri,
    pub position: Position,
}

/// The edits a client applies in place of Enter's own behaviour.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct EnterReply {
    pub edits: Option<Vec<TextEdit>>,
}

pub(super) enum EnterRequest {}

impl LspRequest for EnterRequest {
    type Params = EnterParams;
    type Result = EnterReply;
    const METHOD: &'static str = "tola/onEnter";
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct PackageSourceParams {
    pub uri: Uri,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct PackageSourceText {
    pub text: String,
}

/// Routes realized from the current checked sources, including unsaved editor text.
#[derive(Debug, Default, Deserialize, Serialize)]
pub(super) struct RouteReply {
    pub routes: Vec<Route>,
}

/// A realized document address; this does not establish dev-server publication.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Route {
    /// The logical output path the document is written to.
    pub output: String,
    /// The browser path the document answers at, under the site's base path.
    pub route: String,
    /// The canonical URL, when the site declares its origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The command that opens one realized page, which this server runs for every client.
pub(super) const PREVIEW_COMMAND: &str = "tola.openPreview";

/// The `CodeLens.data` a preview lens has: the route it opens.
///
/// A client with no command support still reads what the lens names, so the lens is a fact about
/// the page rather than a promise only one editor can keep.
pub(super) fn preview_lens_data(route: &str, url: Option<&str>) -> serde_json::Value {
    match url {
        Some(url) => serde_json::json!({ "route": route, "url": url }),
        None => serde_json::json!({ "route": route }),
    }
}

/// The route one preview lens names, which follows the document it belongs to.
///
/// A lens that names no route offers a choice of pages, so the command has the document alone
/// and the client asks which page when it runs.
pub(super) fn preview_route(command: &lsp_types::Command) -> Option<String> {
    if command.command != PREVIEW_COMMAND {
        return None;
    }
    command
        .arguments
        .as_ref()?
        .get(1)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// One `tola.openPreview` request: the document whose lens asked, and the route it names.
#[derive(Debug)]
pub(super) struct PreviewRequest {
    pub uri: Uri,
    /// The route to open; absent when the lens offered a choice, so the answer is the choice.
    pub route: Option<String>,
}

impl PreviewRequest {
    /// The request one command's argument list has, or `None` when it names no document.
    pub(super) fn decode(arguments: &[serde_json::Value]) -> Option<Self> {
        let uri = arguments.first()?.as_str()?.parse().ok()?;
        let route = arguments
            .get(1)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        Some(Self { uri, route })
    }
}

/// What `tola.openPreview` answers.
///
/// An editor that shows no result still receives the sentence, and that sentence names the command
/// that serves the site: this server publishes nothing, so an author whose client cannot be handed
/// an address is told which process serves it rather than left with a failure.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PreviewReply {
    /// The address the page is served at; absent when the server does not know where this site is
    /// served.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Whether the server asked the client to open `url`, which only a client that declared
    /// `window/showDocument` is asked to do.
    pub showing: bool,
    /// What the author does next.
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct RouteParams {
    pub uri: Uri,
}

pub(super) enum RouteRequest {}

impl LspRequest for RouteRequest {
    type Params = RouteParams;
    type Result = RouteReply;
    const METHOD: &'static str = "tola/route";
}

pub(super) enum PackageSourceRequest {}

impl LspRequest for PackageSourceRequest {
    type Params = PackageSourceParams;
    type Result = PackageSourceText;
    const METHOD: &'static str = "tola/source";
}

#[derive(Debug)]
pub(super) enum SourceQuery {
    Completion(TextDocumentPositionParams),
    Hover(TextDocumentPositionParams),
    Definition(TextDocumentPositionParams),
    SignatureHelp(TextDocumentPositionParams),
    PrepareRename(TextDocumentPositionParams),
    Rename(RenameParams),
    References(ReferenceParams),
    DocumentHighlights(DocumentHighlightParams),
    CodeActions(CodeActionParams),
    DocumentColors(DocumentColorParams),
    ColorPresentations(ColorPresentationParams),
    InlayHints(InlayHintParams),
}

impl SourceQuery {
    pub(super) fn decode(request: Request) -> Result<(RequestId, Self), Box<Response>> {
        match request.method.as_str() {
            request::Completion::METHOD => extract::<request::Completion>(request)
                .map(|(id, params)| (id, Self::Completion(params.text_document_position))),
            request::HoverRequest::METHOD => extract::<request::HoverRequest>(request)
                .map(|(id, params)| (id, Self::Hover(params.text_document_position_params))),
            request::GotoDefinition::METHOD => extract::<request::GotoDefinition>(request)
                .map(|(id, params)| (id, Self::Definition(params.text_document_position_params))),
            request::SignatureHelpRequest::METHOD => {
                extract::<request::SignatureHelpRequest>(request).map(|(id, params)| {
                    (
                        id,
                        Self::SignatureHelp(params.text_document_position_params),
                    )
                })
            }
            request::CodeActionRequest::METHOD => extract::<request::CodeActionRequest>(request)
                .map(|(id, params)| (id, Self::CodeActions(params))),
            request::DocumentColor::METHOD => extract::<request::DocumentColor>(request)
                .map(|(id, params)| (id, Self::DocumentColors(params))),
            request::ColorPresentationRequest::METHOD => {
                extract::<request::ColorPresentationRequest>(request)
                    .map(|(id, params)| (id, Self::ColorPresentations(params)))
            }
            request::DocumentHighlightRequest::METHOD => {
                extract::<request::DocumentHighlightRequest>(request)
                    .map(|(id, params)| (id, Self::DocumentHighlights(params)))
            }
            request::PrepareRenameRequest::METHOD => {
                extract::<request::PrepareRenameRequest>(request)
                    .map(|(id, params)| (id, Self::PrepareRename(params)))
            }
            request::Rename::METHOD => {
                extract::<request::Rename>(request).map(|(id, params)| (id, Self::Rename(params)))
            }
            request::References::METHOD => extract::<request::References>(request)
                .map(|(id, params)| (id, Self::References(params))),
            request::InlayHintRequest::METHOD => extract::<request::InlayHintRequest>(request)
                .map(|(id, params)| (id, Self::InlayHints(params))),
            _ => Err(Box::new(Response::new_err(
                request.id,
                ErrorCode::MethodNotFound as i32,
                "method is not supported".into(),
            ))),
        }
    }

    /// The document and cursor position this query selects.
    pub(super) fn document(&self) -> (&Uri, Position) {
        let params = match self {
            Self::Completion(params)
            | Self::Hover(params)
            | Self::Definition(params)
            | Self::SignatureHelp(params)
            | Self::PrepareRename(params) => params,
            Self::Rename(params) => &params.text_document_position,
            Self::DocumentHighlights(params) => &params.text_document_position_params,
            // A fix answers for a viewport, so the range start stands in for the cursor.
            Self::CodeActions(params) => {
                return (&params.text_document.uri, params.range.start);
            }
            Self::DocumentColors(params) => {
                return (&params.text_document.uri, Position::new(0, 0));
            }
            Self::ColorPresentations(params) => {
                return (&params.text_document.uri, params.range.start);
            }
            Self::References(params) => &params.text_document_position,
            // Hints answer for a viewport, so the range start stands in for the cursor.
            Self::InlayHints(params) => {
                return (&params.text_document.uri, params.range.start);
            }
        };
        (&params.text_document.uri, params.position)
    }

    /// Whether this query is answered from the source-local name graph.
    ///
    /// A definition, a rename, its preparation, the references a declaration reaches, and the
    /// highlights of one spelling must find every occurrence, which only the source-local name
    /// graph knows. Every other query is answered from the checked world or the source's own
    /// text alone.
    pub(super) fn answered_from_name_graph(&self) -> bool {
        matches!(
            self,
            Self::Definition(_)
                | Self::PrepareRename(_)
                | Self::Rename(_)
                | Self::References(_)
                | Self::DocumentHighlights(_)
        )
    }
}

/// The quick fix that replaces `range` in `uri` with `new_text`.
pub(super) fn quick_fix(uri: &Uri, title: String, edit: TextEdit) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(std::iter::once((uri.clone(), vec![edit])).collect()),
            ..WorkspaceEdit::default()
        }),
        ..CodeAction::default()
    })
}

/// The markdown hover envelope every hover reply shares.
pub(super) fn markdown_hover(value: String, range: Option<lsp_types::Range>) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range,
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(super) enum SourceReply {
    Completion(CompletionResponse),
    Routes(RouteReply),
    CodeLenses(Option<Vec<CodeLens>>),
    Hover(Option<Hover>),
    Definition(Option<GotoDefinitionResponse>),
    SignatureHelp(Option<SignatureHelp>),
    PrepareRename(Option<PrepareRenameResponse>),
    Rename(Option<WorkspaceEdit>),
    References(Option<Vec<Location>>),
    WorkspaceSymbols(WorkspaceSymbolResponse),
    DocumentHighlights(Option<Vec<DocumentHighlight>>),
    CodeActions(Option<Vec<CodeActionOrCommand>>),
    /// Every page the site realizes, which no request of one document names.
    RouteIndex(RouteIndexReply),
    DocumentColors(Option<Vec<ColorInformation>>),
    ColorPresentations(Option<Vec<ColorPresentation>>),
    InlayHints(Vec<InlayHint>),
    PackageSource(PackageSourceText),
    PrepareCallHierarchy(Option<Vec<CallHierarchyItem>>),
    IncomingCalls(Option<Vec<CallHierarchyIncomingCall>>),
    OutgoingCalls(Option<Vec<CallHierarchyOutgoingCall>>),
}

fn extract<R: LspRequest>(request: Request) -> Result<(RequestId, R::Params), Box<Response>> {
    let id = request.id.clone();
    request.extract(R::METHOD).map_err(|error| {
        Box::new(Response::new_err(
            id,
            ErrorCode::InvalidParams as i32,
            error.to_string(),
        ))
    })
}

/// The action that creates the file a source names, which the author confirms.
///
/// Creating a file is not an edit: the client asks the author before it adds one.
pub(super) fn create_file(path: &std::path::Path) -> Option<CodeActionOrCommand> {
    let uri = crate::uri::from_file_path(path).ok()?;
    const ANNOTATION: &str = "tola-create-file";
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!(
            "create `{}`",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("the file")
        ),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            document_changes: Some(DocumentChanges::Operations(vec![
                DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                    uri,
                    options: Some(CreateFileOptions {
                        overwrite: Some(false),
                        ignore_if_exists: Some(false),
                    }),
                    annotation_id: Some(ANNOTATION.to_owned()),
                })),
            ])),
            change_annotations: Some(
                std::iter::once((
                    ANNOTATION.to_owned(),
                    ChangeAnnotation {
                        label: "create the file this source names".to_owned(),
                        needs_confirmation: Some(true),
                        description: None,
                    },
                ))
                .collect(),
            ),
            ..WorkspaceEdit::default()
        }),
        ..CodeAction::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_rejections_name_their_code() {
        let invalid = SourceQuery::decode(Request::new(
            1.into(),
            request::Completion::METHOD.into(),
            json!({
                "textDocument": { "uri": "file:///site/document.typ" },
                "position": { "line": "wrong", "character": 0 }
            }),
        ))
        .unwrap_err();
        assert_eq!(invalid.id, RequestId::from(1));
        assert_eq!(
            invalid.response_result.unwrap_err().code,
            ErrorCode::InvalidParams as i32
        );

        let unknown = SourceQuery::decode(Request::new(
            "request".to_owned().into(),
            "unknown/method".into(),
            (),
        ))
        .unwrap_err();
        assert_eq!(unknown.id, RequestId::from("request".to_owned()));
        assert_eq!(
            unknown.response_result.unwrap_err().code,
            ErrorCode::MethodNotFound as i32
        );
    }

    /// Every hover reply shares one envelope: markdown contents, and the range the answer covers.
    #[test]
    fn markdown_hover_names_markup_kind_and_range() {
        let range = lsp_types::Range::new(
            lsp_types::Position::new(1, 2),
            lsp_types::Position::new(1, 5),
        );
        let hover = markdown_hover("body".to_owned(), Some(range));
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("markdown contents");
        };
        assert_eq!(markup.kind, MarkupKind::Markdown);
        assert_eq!(markup.value, "body");
        assert_eq!(hover.range, Some(range));
    }
}

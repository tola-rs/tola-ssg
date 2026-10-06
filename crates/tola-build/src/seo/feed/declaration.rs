//! One validated feed and its explicitly selected entries.

use std::collections::BTreeSet;

use typst::foundations::Value;

use super::{
    FeedFormat,
    content::{FeedContent, FeedContentError},
    document::DocumentContents,
};
use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::config::ResolvedSiteConfig;
use crate::seo::RenderError;
use crate::seo::declaration::{
    SeoDeclaration, SeoDeclarationError, field_path, index_path, published_date, resolve_target,
};
use tola_address::OutputPath;

pub(super) struct Author {
    pub(super) name: String,
    pub(super) email: Option<String>,
    pub(super) url: Option<String>,
}

pub(super) struct Feed {
    pub(super) output: OutputPath,
    pub(super) format: FeedFormat,
    pub(super) id: String,
    pub(super) url: String,
    pub(super) home_url: String,
    pub(super) title: String,
    pub(super) description: String,
    pub(super) language: String,
    pub(super) authors: Vec<Author>,
    pub(super) entries: Vec<FeedEntry>,
}

pub(super) struct FeedEntry {
    pub(super) id: String,
    pub(super) url: String,
    pub(super) title: String,
    pub(super) published: atom_syndication::FixedDateTime,
    pub(super) updated: Option<atom_syndication::FixedDateTime>,
    pub(super) summary: Option<EntryText>,
    pub(super) content: Option<EntryContent>,
    pub(super) authors: Vec<Author>,
}

pub(super) enum EntryText {
    Text(String),
    Rich(FeedContent),
}

pub(super) enum EntryContent {
    Authored(EntryText),
    Document(std::sync::Arc<str>),
}

impl EntryContent {
    pub(super) fn html(
        &self,
        page_url: &str,
        cancellation: &BuildCancellation,
    ) -> Result<String, BuildCancelled> {
        cancellation.ensure_active()?;
        match self {
            Self::Authored(text) => text.html(page_url, cancellation),
            Self::Document(html) => Ok(html.to_string()),
        }
    }
}

impl EntryText {
    pub(super) fn plain_text(
        &self,
        cancellation: &BuildCancellation,
    ) -> Result<String, BuildCancelled> {
        cancellation.ensure_active()?;
        match self {
            Self::Text(text) => Ok(text.clone()),
            Self::Rich(content) => content.to_plain_text(cancellation),
        }
    }

    pub(super) fn html(
        &self,
        page_url: &str,
        cancellation: &BuildCancellation,
    ) -> Result<String, BuildCancelled> {
        cancellation.ensure_active()?;
        match self {
            Self::Text(text) => Ok(crate::html::escape(text).into_owned()),
            Self::Rich(content) => content.to_html_with_links(
                |destination| super::html::absolute_url(destination, page_url),
                cancellation,
            ),
        }
    }
}

impl Feed {
    pub(super) fn parse(
        declaration: &SeoDeclaration,
        config: &ResolvedSiteConfig,
        compilation: &tola_typst::BundleCompilation,
        cancellation: &BuildCancellation,
        document_contents: &mut DocumentContents<'_>,
    ) -> Result<Self, RenderError> {
        cancellation.ensure_active()?;
        let fields = &declaration.fields;
        declaration.check_fields(
            fields,
            &[
                "output",
                "format",
                "id",
                "title",
                "description",
                "language",
                "authors",
                "entries",
            ],
            "",
        )?;
        declaration.require_origin(config)?;
        let output = declaration.output()?;
        let format = FeedFormat::from_authored_name(
            &declaration.string(declaration.required(fields, "format", "")?, "/format")?,
        )
        .ok_or_else(|| declaration.invalid("/format", "expected `rss`, `atom`, or `json`"))?;
        let output_url = tola_address::asset_url_from_output(&output);
        let url = config.canonical_url(&output_url);
        let id = declaration.text(declaration.required(fields, "id", "")?, "/id")?;
        let title = declaration.text(declaration.required(fields, "title", "")?, "/title")?;
        if title.trim().is_empty() {
            return Err(declaration
                .invalid(
                    "/title",
                    "feed title must not be blank; set `title` or `site.title`",
                )
                .into());
        }
        let description = declaration.text(
            declaration.required(fields, "description", "")?,
            "/description",
        )?;
        let language =
            declaration.text(declaration.required(fields, "language", "")?, "/language")?;
        let authors = parse_authors(
            declaration,
            Some(declaration.required(fields, "authors", "")?),
            "/authors",
            cancellation,
        )?;
        let values = declaration.required_array(fields, "entries", "")?;
        let mut entries = Vec::with_capacity(values.len());
        let mut entry_ids = BTreeSet::new();
        for (index, value) in values.iter().enumerate() {
            cancellation.ensure_active()?;
            let entry_path = index_path("/entries", index);
            let Value::Dict(fields) = value else {
                return Err(declaration
                    .expected(&entry_path, "dictionary", value)
                    .into());
            };
            declaration.check_fields(
                fields,
                &[
                    "id",
                    "target",
                    "title",
                    "published",
                    "updated",
                    "summary",
                    "content",
                    "authors",
                ],
                &entry_path,
            )?;
            let target = resolve_target(
                declaration,
                declaration.required(fields, "target", &entry_path)?,
                &field_path(&entry_path, "target"),
                &output,
                config,
                compilation,
                cancellation,
            )?;
            let url: String = target.url.into();
            let id_path = field_path(&entry_path, "id");
            let id = match fields.get("id").ok() {
                None | Some(Value::Auto) => url.clone(),
                Some(value) => declaration.string(value, &id_path)?,
            };
            if id.trim().is_empty() {
                return Err(declaration
                    .invalid(&id_path, "entry id must not be blank")
                    .into());
            }
            if !entry_ids.insert(id.clone()) {
                return Err(declaration
                    .invalid(&id_path, format!("duplicate entry id `{id}` in this feed"))
                    .into());
            }
            if format == FeedFormat::Atom {
                validate_atom_id(declaration, &id, &id_path)?;
            }
            let title = match fields.get("title").ok() {
                None | Some(Value::Auto) => target.title.ok_or_else(|| {
                    declaration.invalid(
                        &field_path(&entry_path, "title"),
                        format!(
                            "target document `{}` has no title; set this entry's title",
                            target.output
                        ),
                    )
                })?,
                Some(value) => declaration.text(value, &field_path(&entry_path, "title"))?,
            };
            let published = published_date(
                declaration,
                declaration.required(fields, "published", &entry_path)?,
                &field_path(&entry_path, "published"),
            )?;
            let updated = match fields.get("updated").ok() {
                None | Some(Value::None) => None,
                Some(value) => Some(published_date(
                    declaration,
                    value,
                    &field_path(&entry_path, "updated"),
                )?),
            };
            let summary = parse_entry_text(
                declaration,
                fields.get("summary").ok(),
                &field_path(&entry_path, "summary"),
                cancellation,
            )?;
            let content_field = field_path(&entry_path, "content");
            let content = match fields.get("content").ok() {
                Some(Value::Dict(fields)) => Some(EntryContent::Document(document_contents.load(
                    declaration,
                    fields,
                    &content_field,
                    &output,
                    cancellation,
                )?)),
                value => parse_entry_text(declaration, value, &content_field, cancellation)?
                    .map(EntryContent::Authored),
            };
            let authors = parse_authors(
                declaration,
                fields.get("authors").ok(),
                &field_path(&entry_path, "authors"),
                cancellation,
            )?;
            entries.push(FeedEntry {
                id,
                url,
                title,
                published,
                updated,
                summary,
                content,
                authors,
            });
        }
        if format == FeedFormat::Atom {
            validate_atom_id(declaration, &id, "/id")?;
            if authors.is_empty()
                && (entries.is_empty() || entries.iter().any(|entry| entry.authors.is_empty()))
            {
                return Err(declaration
                    .invalid(
                        "/authors",
                        "Atom requires feed authors or an author on every entry",
                    )
                    .into());
            }
        }
        Ok(Self {
            output,
            format,
            id,
            url,
            home_url: config.canonical_url(&tola_address::UrlPath::default()),
            title,
            description,
            language,
            authors,
            entries,
        })
    }
}

fn validate_atom_id(
    declaration: &SeoDeclaration,
    id: &str,
    field: &str,
) -> Result<(), SeoDeclarationError> {
    if id.chars().any(char::is_whitespace) || url::Url::parse(id).is_err() {
        return Err(declaration.invalid(
            field,
            "Atom id must be an absolute URI without whitespace, such as urn:notes:entry-1",
        ));
    }
    Ok(())
}

fn parse_entry_text(
    declaration: &SeoDeclaration,
    value: Option<&Value>,
    field: &str,
    cancellation: &BuildCancellation,
) -> Result<Option<EntryText>, RenderError> {
    cancellation.ensure_active()?;
    match value {
        None | Some(Value::None) => Ok(None),
        Some(Value::Str(text)) => Ok(Some(EntryText::Text(text.to_string()))),
        Some(Value::Content(content)) => {
            FeedContent::from_typst(content.clone(), field, cancellation)
                .map(EntryText::Rich)
                .map(Some)
                .map_err(|error| match error {
                    FeedContentError::Cancelled(cancelled) => RenderError::Cancelled(cancelled),
                    FeedContentError::Invalid { path, violation } => {
                        declaration.content_violation(&path, violation).into()
                    }
                })
        }
        Some(value) => Err(declaration
            .expected(field, "string, supported rich content, or none", value)
            .into()),
    }
}

fn parse_authors(
    declaration: &SeoDeclaration,
    value: Option<&Value>,
    field: &str,
    cancellation: &BuildCancellation,
) -> Result<Vec<Author>, RenderError> {
    cancellation.ensure_active()?;
    let values = match value {
        None | Some(Value::None) => return Ok(Vec::new()),
        Some(Value::Array(authors)) => authors.as_slice(),
        Some(value) => std::slice::from_ref(value),
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            cancellation.ensure_active()?;
            let path = index_path(field, index);
            let author = match value {
                Value::Str(name) => Author {
                    name: name.to_string(),
                    email: None,
                    url: None,
                },
                Value::Dict(fields) => {
                    declaration.check_fields(fields, &["name", "email", "url"], &path)?;
                    let name = declaration.string(
                        declaration.required(fields, "name", &path)?,
                        &field_path(&path, "name"),
                    )?;
                    Author {
                        name,
                        email: declaration.optional_string(fields, "email", &path)?,
                        url: declaration.optional_string(fields, "url", &path)?,
                    }
                }
                value => {
                    return Err(declaration
                        .expected(&path, "string or author dictionary", value)
                        .into());
                }
            };
            if author.name.trim().is_empty() {
                return Err(declaration
                    .invalid(&path, "author name must not be blank")
                    .into());
            }
            Ok(author)
        })
        .collect()
}

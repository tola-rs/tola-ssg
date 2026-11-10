//! Browser references and the site targets they require.

use tola_address::{LinkKind, RequiredResourceKind, SiteReference, SiteUrlMount};
use tola_typst::HtmlReference;

use crate::output::semantics::ResponseMediaType;
use crate::site::space::AddressDependencies;
use crate::site::{AddressResolution, AddressSpace, HtmlPage, Resource};

use super::{
    Reference, ReferenceCategory, ReferenceResolution, ReferenceTarget, UnresolvedReferenceReason,
};

pub(super) fn read_reference(
    address: &AddressSpace,
    page: &HtmlPage,
    base: Option<&str>,
    reference: &HtmlReference,
    url_mount: &SiteUrlMount,
    world: &tola_typst::TypstWorld,
    dependencies: &mut AddressDependencies,
) -> Reference {
    let destination = reference.destination();
    let effective =
        tola_address::resolve_browser_reference(&page.permalink, url_mount, base, destination);
    let expectation = expectation(reference.reference_use());
    let default_category = expectation.category;
    let resolution = match effective {
        Ok(tola_address::ResolvedBrowserReference::External) => ReferenceResolution::External,
        Ok(tola_address::ResolvedBrowserReference::OutsideSite(destination)) => {
            ReferenceResolution::Unresolved {
                reason: UnresolvedReferenceReason::OutsideSiteMount {
                    destination,
                    required_prefix: format!("/{}/", url_mount.as_str()),
                },
            }
        }
        Err(_) => ReferenceResolution::Unresolved {
            reason: UnresolvedReferenceReason::InvalidBrowserReference {
                message: "is not a valid URL".to_string(),
            },
        },
        Ok(tola_address::ResolvedBrowserReference::Site(site)) => {
            match lookup_target(
                address,
                site,
                reference,
                expectation,
                url_mount,
                dependencies,
            ) {
                Ok(target) => ReferenceResolution::Found { target },
                Err(reason) => ReferenceResolution::Unresolved { reason },
            }
        }
    };
    let category = match &resolution {
        ReferenceResolution::External => ReferenceCategory::External,
        ReferenceResolution::Unresolved {
            reason: UnresolvedReferenceReason::FragmentMissing { .. },
        } => ReferenceCategory::Fragment,
        ReferenceResolution::Found { .. }
            if matches!(LinkKind::parse(destination), LinkKind::Fragment(_)) =>
        {
            ReferenceCategory::Fragment
        }
        _ => default_category,
    };

    Reference {
        page: page.output.clone(),
        destination: destination.to_owned(),
        html_context: reference_context(reference),
        category,
        resolution,
        origin: crate::compiler::source_location(world, reference.span()),
    }
}

/// Category, destination constraint, media requirement, and wording of one reference use.
pub(super) struct ReferenceExpectation {
    category: ReferenceCategory,
    target: RequiredResourceKind,
    media_type: Option<ResponseMediaType>,
    pub(super) usage: &'static str,
}

/// What each way of using a reference requires.
pub(super) const fn expectation(
    reference_use: tola_typst::HtmlReferenceUse,
) -> ReferenceExpectation {
    use tola_typst::HtmlReferenceUse as HtmlUse;
    match reference_use {
        HtmlUse::Navigation => ReferenceExpectation {
            category: ReferenceCategory::Navigation,
            target: RequiredResourceKind::DocumentOrResource,
            media_type: None,
            usage: "a navigation link",
        },
        HtmlUse::GenericResource => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::NonDocumentResource,
            media_type: None,
            usage: "an embedded resource",
        },
        HtmlUse::Prefetch => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::DocumentOrResource,
            media_type: None,
            usage: "a prefetch link",
        },
        HtmlUse::Stylesheet => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::NonDocumentResource,
            media_type: Some(ResponseMediaType::CSS),
            usage: "a stylesheet",
        },
        HtmlUse::ClassicScript => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::NonDocumentResource,
            media_type: Some(ResponseMediaType::JAVASCRIPT),
            usage: "a classic script",
        },
        HtmlUse::ModuleScript => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::NonDocumentResource,
            media_type: Some(ResponseMediaType::JAVASCRIPT),
            usage: "a module script",
        },
        HtmlUse::ModulePreload => ReferenceExpectation {
            category: ReferenceCategory::Resource,
            target: RequiredResourceKind::NonDocumentResource,
            media_type: Some(ResponseMediaType::JAVASCRIPT),
            usage: "a module preload link",
        },
    }
}

fn lookup_target(
    address: &AddressSpace,
    site: SiteReference,
    reference: &HtmlReference,
    expectation: ReferenceExpectation,
    url_mount: &SiteUrlMount,
    dependencies: &mut AddressDependencies,
) -> Result<ReferenceTarget, UnresolvedReferenceReason> {
    let reference_use = reference.reference_use();
    let allows_document = matches!(reference_use, tola_typst::HtmlReferenceUse::GenericResource)
        && matches!(
            (reference.tag(), reference.attribute()),
            ("object", "data") | ("embed", "src") | ("a", "ping") | ("area", "ping")
        );
    let required_kind = if allows_document {
        RequiredResourceKind::DocumentOrResource
    } else {
        expectation.target
    };

    let target = match address.resolve_route(
        &site.route,
        site.fragment.as_deref().unwrap_or_default(),
        required_kind,
        &mut *dependencies,
    ) {
        AddressResolution::Found { url } => ReferenceTarget::new(url, site),
        AddressResolution::NotFound => {
            let suggestion = address
                .directory_page_suggestion(&site.route, required_kind, &mut *dependencies)
                .map(|route| SiteReference { route, ..site }.to_browser_url(url_mount, None));
            return Err(UnresolvedReferenceReason::TargetMissing { suggestion });
        }
        AddressResolution::FragmentNotFound {
            fragment,
            available,
        } => {
            return Err(UnresolvedReferenceReason::FragmentMissing {
                fragment,
                available,
            });
        }
        AddressResolution::DocumentNotAllowed {
            reference,
            document,
        } => {
            return Err(UnresolvedReferenceReason::DocumentNotAllowed {
                reference,
                document,
            });
        }
    };

    let Some(required) = expectation.media_type else {
        return Ok(target);
    };
    let Some(Resource::Asset { route }) = address.get_by_url(target.url()) else {
        unreachable!("a media-typed reference resolves to a registered asset");
    };
    let declared = route.declaration().media_type();
    dependencies.observe_media_type(target.url(), declared);
    if !resource_media_matches(declared, &required) {
        return Err(UnresolvedReferenceReason::MediaTypeMismatch {
            reference_use,
            required,
            declared: declared.clone(),
        });
    }
    Ok(target)
}

fn resource_media_matches(declared: &ResponseMediaType, required: &ResponseMediaType) -> bool {
    if required.essence() != ResponseMediaType::JAVASCRIPT.essence() {
        return declared.essence() == required.essence();
    }
    tola_typst::html::is_javascript_mime_type(declared.essence())
}

fn reference_context(reference: &HtmlReference) -> String {
    match reference.relation() {
        Some(relation) => format!(
            "{}[{} rel={relation}]",
            reference.tag(),
            reference.attribute()
        ),
        None => format!("{}[{}]", reference.tag(), reference.attribute()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_media_matches_by_essence() {
        let stylesheet = ResponseMediaType::parse("text/css; charset=iso-8859-1").unwrap();
        assert!(resource_media_matches(&stylesheet, &ResponseMediaType::CSS));
        let script = ResponseMediaType::parse("application/javascript; charset=utf-8").unwrap();
        assert!(resource_media_matches(
            &script,
            &ResponseMediaType::JAVASCRIPT
        ));
        assert!(!resource_media_matches(
            &stylesheet,
            &ResponseMediaType::JAVASCRIPT
        ));
        let track = ResponseMediaType::parse("text/vtt").unwrap();
        assert!(!resource_media_matches(&track, &ResponseMediaType::CSS));
    }
}

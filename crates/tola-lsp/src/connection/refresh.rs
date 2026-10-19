//! Refreshing client token displays after a completed source revision.

use std::io::Write;

use anyhow::Result;
use lsp_server::{RequestId, Response};
use lsp_types::request::{Request as _, SemanticTokensRefresh};
use tola_build::diagnostic::Diagnostic;

use super::{Connection, Phase};

/// One request may await the client; newer completed work is read from `Connection::checked`.
#[derive(Default)]
pub(super) struct SemanticRefresh {
    in_flight: Option<RequestId>,
    sent_revision: Option<u64>,
}

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    pub(super) fn refresh_semantic_tokens(&mut self, checked_revision: u64) -> Result<()> {
        if self.revision != checked_revision
            || self.checking.token().is_cancelled()
            || self.shutdown.token().is_cancelled()
            || !self
                .checked
                .is_some_and(|(revision, _)| revision == checked_revision)
            || self.tokens.is_empty()
            || self.semantic_refresh.in_flight.is_some()
            || self.semantic_refresh.sent_revision == Some(checked_revision)
            || !matches!(&self.phase, Phase::Ready(workspace) if workspace.features.semantic_tokens_refresh)
        {
            return Ok(());
        }
        let id = self.send_request(SemanticTokensRefresh::METHOD, ())?;
        self.semantic_refresh.in_flight = Some(id);
        self.semantic_refresh.sent_revision = Some(checked_revision);
        Ok(())
    }

    pub(super) fn semantic_tokens_refreshed(&mut self, response: &Response) -> Result<bool> {
        if self.semantic_refresh.in_flight.as_ref() != Some(&response.id) {
            return Ok(false);
        }
        self.semantic_refresh.in_flight = None;
        if response.response_result.is_ok()
            && let Some((revision, _)) = self.checked
        {
            self.refresh_semantic_tokens(revision)?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::ClientFeatures;
    use crate::connection::tests::{messages, ready, ready_workspace};
    use crate::protocol::CheckMode;
    use lsp_server::Message;
    use serde_json::json;

    fn token_client(supported: bool) -> Connection<Vec<u8>, impl Fn(&[Diagnostic])> {
        let mut connection = ready(Vec::new());
        ready_workspace(&mut connection).features = ClientFeatures::new(
            &serde_json::from_value(
                json!({"workspace":{"semanticTokens":{"refreshSupport":supported}}}),
            )
            .unwrap(),
        );
        connection
            .tokens
            .insert("untitled:Tokens".parse().unwrap(), Some(Default::default()));
        connection.revision = 1;
        connection.checked = Some((1, true));
        connection
    }

    #[test]
    fn refreshes_coalesce_completed_revisions() {
        let mut connection = token_client(true);
        connection.refresh_semantic_tokens(1).unwrap();
        let written = messages(&mut connection);
        let [Message::Request(first)] = written.as_slice() else {
            panic!("one refresh request: {written:?}");
        };
        assert_eq!(first.method, SemanticTokensRefresh::METHOD);
        connection.refresh_semantic_tokens(1).unwrap();
        assert!(messages(&mut connection).is_empty());
        for revision in [2, 3] {
            connection.revision = revision;
            connection.checked = Some((revision, true));
            connection.refresh_semantic_tokens(revision).unwrap();
            assert!(messages(&mut connection).is_empty());
        }
        connection
            .response(Response::new_ok(first.id.clone(), ()))
            .unwrap();
        let written = messages(&mut connection);
        let [Message::Request(latest)] = written.as_slice() else {
            panic!("one merged refresh request: {written:?}");
        };
        assert_ne!(latest.id, first.id);
        assert_eq!(connection.semantic_refresh.sent_revision, Some(3));
        connection
            .response(Response::new_ok(latest.id.clone(), ()))
            .unwrap();
        assert!(messages(&mut connection).is_empty());
    }

    #[test]
    fn refreshes_wait_for_saved_revisions() {
        let mut connection = token_client(true);
        ready_workspace(&mut connection).check_mode = CheckMode::OnSave;
        connection.refresh_semantic_tokens(1).unwrap();
        let written = messages(&mut connection);
        let [Message::Request(first)] = written.as_slice() else {
            panic!("one refresh request");
        };
        connection.sources_changed(true).unwrap();
        connection
            .response(Response::new_ok(first.id.clone(), ()))
            .unwrap();
        assert!(messages(&mut connection).is_empty());
        connection.sources_changed(false).unwrap();
        connection.checked = Some((connection.revision, true));
        connection
            .refresh_semantic_tokens(connection.revision)
            .unwrap();
        assert!(matches!(
            messages(&mut connection).as_slice(),
            [Message::Request(_)]
        ));
    }

    #[test]
    fn refreshes_require_supported_token_displays() {
        for supported in [false, true] {
            for baseline in [false, true] {
                let mut connection = token_client(supported);
                if !baseline {
                    connection.tokens.clear();
                }
                connection.refresh_semantic_tokens(1).unwrap();
                assert_eq!(
                    messages(&mut connection).len(),
                    usize::from(supported && baseline)
                );
            }
        }
    }

    #[test]
    fn range_displays_refresh_without_full_baselines() {
        let mut connection = token_client(true);
        connection.tokens.clear();
        let uri: lsp_types::Uri = "untitled:Range".parse().unwrap();
        connection
            .commit_tokens(
                RequestId::from(99),
                crate::compiler::TokensRead {
                    uri: uri.clone(),
                    kind: crate::compiler::TokensKind::Range(lsp_types::Range::default()),
                },
                Default::default(),
            )
            .unwrap();
        assert!(connection.tokens.get(&uri).is_some_and(Option::is_none));
        assert!(matches!(
            messages(&mut connection).as_slice(),
            [Message::Response(_)]
        ));
        connection.refresh_semantic_tokens(1).unwrap();
        let written = messages(&mut connection);
        assert!(matches!(
            written.as_slice(),
            [Message::Request(request)] if request.method == SemanticTokensRefresh::METHOD
        ));
    }

    #[test]
    fn closed_displays_drop_pending_refreshes() {
        let mut connection = token_client(true);
        connection.refresh_semantic_tokens(1).unwrap();
        let written = messages(&mut connection);
        let [Message::Request(first)] = written.as_slice() else {
            panic!("one refresh request");
        };
        connection.revision = 2;
        connection.checked = Some((2, true));
        connection.refresh_semantic_tokens(2).unwrap();
        connection.tokens.clear();
        connection
            .response(Response::new_ok(first.id.clone(), ()))
            .unwrap();
        assert!(connection.semantic_refresh.in_flight.is_none());
        assert!(messages(&mut connection).is_empty());
    }
}

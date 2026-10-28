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
    /// A refresh is asked of a client that declared `refreshSupport` and holds a baseline to
    /// refresh: every other capability state is answered with silence.
    #[test]
    fn refreshes_need_supported_token_displays() {
        // Rows: (client declares refreshSupport, holds a full-token baseline, requests written).
        for (supported, baseline, expected) in [
            (false, true, 0),
            (true, false, 0),
            (false, false, 0),
            (true, true, 1),
        ] {
            let mut connection = token_client(supported);
            if !baseline {
                connection.tokens.clear();
            }
            connection.refresh_semantic_tokens(1).unwrap();
            assert_eq!(
                messages(&mut connection).len(),
                expected,
                "refreshSupport={supported}, baseline={baseline}"
            );
        }
    }
}

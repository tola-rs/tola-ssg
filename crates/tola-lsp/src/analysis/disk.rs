//! The site's own Typst files as one lane last read them.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tola_typst::ContentDigest;
use tola_typst::typst::syntax::{FileId, Source};
use tola_typst_syntax::names::SourceNames;

/// The site's own Typst files as one lane last read them, reused while each file's bytes are
/// unchanged.
///
/// References and rename read every file the site holds, and parsing them again for each request
/// costs more than the answer. Each pass re-reads the bytes — the read is what decides whether
/// they changed, identified by the same content digest a compiler read records — and a file the
/// walk no longer sees is dropped.
#[derive(Default)]
pub(crate) struct DiskSources {
    pass: u64,
    parsed: HashMap<FileId, ParsedSource>,
}

struct ParsedSource {
    /// The pass that last read the file, so a file the walk stopped seeing is dropped.
    pass: u64,
    /// The digest of the bytes this parse came from.
    digest: ContentDigest,
    names: Arc<SourceNames>,
}

impl DiskSources {
    /// Start one pass over the site, which a completed walk closes with [`Self::retain_pass`].
    ///
    /// A pass aborted between reads — a cancellation inside the walk — stays open: what it parsed
    /// is kept, and the next completed pass re-reads each file's bytes and drops what it no longer
    /// sees.
    pub(crate) fn begin(&mut self) {
        self.pass += 1;
    }

    /// The parsed names of one site file, parsed again when its bytes changed.
    pub(crate) fn names(&mut self, path: &Path, id: FileId) -> Option<Arc<SourceNames>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                tracing::debug!(
                    path = %path.display(),
                    %error,
                    "a site file the walk reached could not be read"
                );
                return None;
            }
        };
        let digest = ContentDigest::of(text.as_bytes());
        if let Some(parsed) = self.parsed.get_mut(&id)
            && parsed.digest == digest
        {
            parsed.pass = self.pass;
            return Some(Arc::clone(&parsed.names));
        }
        let names = Arc::new(SourceNames::new(Source::new(id, text)));
        self.parsed.insert(
            id,
            ParsedSource {
                pass: self.pass,
                digest,
                names: Arc::clone(&names),
            },
        );
        Some(names)
    }

    /// Close the pass [`Self::begin`] opened: drop every file this pass did not read.
    pub(crate) fn retain_pass(&mut self) {
        let pass = self.pass;
        self.parsed.retain(|_, parsed| parsed.pass == pass);
    }
}

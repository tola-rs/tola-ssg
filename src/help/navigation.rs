//! History retains document positions; terminal rows belong to the current layout only.

use std::collections::VecDeque;
use std::rc::Rc;

use super::layout::DocumentPosition;
use super::model::HelpDocument;

pub(super) struct Visit {
    pub document: Rc<HelpDocument>,
    pub position: DocumentPosition,
    pub focus: Option<DocumentPosition>,
}

#[derive(Default)]
pub(super) struct History {
    back: VecDeque<Visit>,
    forward: Vec<Visit>,
}

impl History {
    // A new visit evicts the oldest one. Back/forward moves retain this combined bound.
    const LIMIT: usize = 128;

    pub fn push(&mut self, visit: Visit) {
        self.forward.clear();
        if self.back.len() == Self::LIMIT {
            self.back.pop_front();
        }
        self.back.push_back(visit);
    }

    pub fn back(&mut self, current: Visit) -> Option<Visit> {
        let visit = self.back.pop_back()?;
        self.forward.push(current);
        Some(visit)
    }

    pub fn forward(&mut self, current: Visit) -> Option<Visit> {
        let visit = self.forward.pop()?;
        self.back.push_back(current);
        Some(visit)
    }

    pub fn has_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn has_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

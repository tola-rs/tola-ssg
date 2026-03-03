//! Page metadata iteration helpers.

use rustc_hash::FxHashSet;

/// Iteration decision after comparing current and previous hash values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StabilityDecision {
    /// State changed; continue iterating.
    Continue,
    /// State converged (hash unchanged).
    Converged,
    /// A previously seen hash reappeared (cycle detected).
    Oscillating,
    /// Reached maximum allowed iterations without convergence.
    MaxIterationsReached,
}

/// Tracks metadata-hash stability across iterative passes.
#[derive(Debug, Clone)]
pub struct HashStabilityTracker {
    prev_hash: u64,
    seen_hashes: FxHashSet<u64>,
    detect_oscillation: bool,
}

impl HashStabilityTracker {
    /// Enable convergence and oscillation detection.
    pub fn with_oscillation_detection(initial_hash: u64) -> Self {
        let mut seen_hashes = FxHashSet::default();
        seen_hashes.insert(initial_hash);
        Self {
            prev_hash: initial_hash,
            seen_hashes,
            detect_oscillation: true,
        }
    }

    /// Enable convergence detection only (no cycle detection).
    pub fn without_oscillation_detection(initial_hash: u64) -> Self {
        Self {
            prev_hash: initial_hash,
            seen_hashes: FxHashSet::default(),
            detect_oscillation: false,
        }
    }

    /// Evaluate the current iteration result.
    ///
    /// `iteration` is zero-based.
    pub fn decide(
        &mut self,
        new_hash: u64,
        iteration: usize,
        max_iterations: usize,
    ) -> StabilityDecision {
        if new_hash == self.prev_hash {
            return StabilityDecision::Converged;
        }

        if self.detect_oscillation && self.seen_hashes.contains(&new_hash) {
            return StabilityDecision::Oscillating;
        }

        if iteration + 1 >= max_iterations {
            return StabilityDecision::MaxIterationsReached;
        }

        if self.detect_oscillation {
            self.seen_hashes.insert(new_hash);
        }
        self.prev_hash = new_hash;
        StabilityDecision::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_convergence() {
        let mut tracker = HashStabilityTracker::with_oscillation_detection(7);
        assert_eq!(tracker.decide(7, 0, 5), StabilityDecision::Converged);
    }

    #[test]
    fn detects_oscillation() {
        let mut tracker = HashStabilityTracker::with_oscillation_detection(1);
        assert_eq!(tracker.decide(2, 0, 5), StabilityDecision::Continue);
        assert_eq!(tracker.decide(1, 1, 5), StabilityDecision::Oscillating);
    }

    #[test]
    fn detects_max_iterations() {
        let mut tracker = HashStabilityTracker::without_oscillation_detection(10);
        assert_eq!(
            tracker.decide(11, 0, 1),
            StabilityDecision::MaxIterationsReached
        );
    }
}

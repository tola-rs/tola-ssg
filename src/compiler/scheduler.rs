//! Compile scheduler with priority queue.
//!
//! Single entry point for all compilation requests:
//! - On-demand (user request) -> Active priority
//! - Hot-reload -> Direct/Affected priority
//! - Background build -> Background priority

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, LazyLock};

use crossbeam::channel::{self, Sender};
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use parking_lot::{Condvar, Mutex};

use crate::address::SiteIndex;
use crate::config::SiteConfig;
use crate::core::{BuildMode, Priority};

// =============================================================================
// Public API
// =============================================================================

/// Global scheduler instance
pub static SCHEDULER: LazyLock<CompileScheduler> = LazyLock::new(CompileScheduler::new);

/// Result of a compilation
#[derive(Debug, Clone)]
pub enum CompileResult {
    Success(PathBuf),
    Failed(String),
    Skipped,
}

// =============================================================================
// Scheduler
// =============================================================================

/// Central compile scheduler with priority queue and deduplication
pub struct CompileScheduler {
    /// Pending tasks split by interactive vs background lanes
    queue: Mutex<QueueState>,
    /// Pending paths -> waiters (for dedup and result broadcasting)
    pending: DashMap<PathBuf, PendingState>,
    /// In-progress paths -> waiters
    active: DashMap<PathBuf, Vec<Waiter>>,
    /// Completed paths -> cached results
    cache: DashMap<PathBuf, CompileResult>,
    /// Worker notification
    notify: Condvar,
    /// Shutdown flag
    shutdown: AtomicBool,
    /// Workers started flag
    started: AtomicBool,
    /// Maximum number of background tasks allowed to execute concurrently.
    /// Reserves at least one worker slot for interactive compiles when possible.
    background_limit: AtomicUsize,
}

type Waiter = Sender<CompileResult>;

struct QueueState {
    foreground: BinaryHeap<Task>,
    background: BinaryHeap<Task>,
    active_background: usize,
}

struct Task {
    path: PathBuf,
    priority: Priority,
}

struct CompileJob {
    path: PathBuf,
    priority: Priority,
    config: Arc<SiteConfig>,
    state: Arc<SiteIndex>,
}

struct PendingState {
    priority: Priority,
    config: Arc<SiteConfig>,
    state: Arc<SiteIndex>,
    waiters: Vec<Waiter>,
}

// Task ordering: higher priority first (BinaryHeap is max-heap)
impl Ord for Task {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority.cmp(&other.priority)
    }
}
impl PartialOrd for Task {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Task {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
    }
}
impl Eq for Task {}

// =============================================================================
// Public methods
// =============================================================================

impl CompileScheduler {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(QueueState {
                foreground: BinaryHeap::new(),
                background: BinaryHeap::new(),
                active_background: 0,
            }),
            pending: DashMap::new(),
            active: DashMap::new(),
            cache: DashMap::new(),
            notify: Condvar::new(),
            shutdown: AtomicBool::new(false),
            started: AtomicBool::new(false),
            background_limit: AtomicUsize::new(1),
        }
    }

    /// Start worker threads.
    pub fn start_workers(&self) {
        if self.started.swap(true, AtomicOrdering::SeqCst) {
            return;
        }
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        self.background_limit
            .store(Self::background_worker_limit(n), AtomicOrdering::SeqCst);
        for _ in 0..n {
            std::thread::spawn(|| SCHEDULER.run_worker());
        }
    }

    /// Request compilation, wait for result.
    pub fn compile(
        &self,
        path: PathBuf,
        priority: Priority,
        config: Arc<SiteConfig>,
        state: Arc<SiteIndex>,
    ) -> CompileResult {
        // Fast path: cached
        if let Some(result) = self
            .get_cached(&path)
            .and_then(Self::reusable_cached_result)
        {
            return result;
        }

        let (tx, rx) = channel::bounded(1);

        // Try join existing work
        if self.try_join_active(&path, tx.clone()) {
            return Self::recv(rx);
        }

        // Atomically join or create pending
        if self.join_or_create_pending(&path, priority, config, state, tx) {
            self.enqueue(path, priority);
        }

        Self::recv(rx)
    }

    /// Submit paths for background compilation (fire-and-forget).
    pub fn submit_background(
        &self,
        paths: Vec<PathBuf>,
        config: Arc<SiteConfig>,
        state: Arc<SiteIndex>,
    ) {
        let mut queue = self.queue.lock();
        for path in paths {
            if self.is_known(&path) {
                continue;
            }
            self.pending.insert(
                path.clone(),
                PendingState {
                    priority: Priority::Background,
                    config: Arc::clone(&config),
                    state: Arc::clone(&state),
                    waiters: vec![],
                },
            );
            queue.background.push(Task {
                path,
                priority: Priority::Background,
            });
        }
        self.notify.notify_all();
    }

    /// Invalidate cache (on file change).
    pub fn invalidate(&self, path: &Path) {
        self.cache.remove(path);
        crate::freshness::invalidate_cached_hash(path);
    }

    /// Check if compiled.
    pub fn is_compiled(&self, path: &Path) -> bool {
        self.cache.contains_key(path)
    }

    /// Get cached result.
    pub fn get_cached(&self, path: &Path) -> Option<CompileResult> {
        self.cache.get(path).map(|r| r.clone())
    }

    /// Clear all cached compile results.
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Signal shutdown.
    pub fn shutdown(&self) {
        self.shutdown.store(true, AtomicOrdering::SeqCst);
        self.notify.notify_all();
    }

    /// Drain all cached failures (path, error message).
    /// Used after initial build to report errors that occurred during background compilation.
    pub fn drain_failures(&self) -> Vec<(PathBuf, String)> {
        let mut failures = Vec::new();
        self.cache.retain(|path, result| {
            if let CompileResult::Failed(msg) = result {
                failures.push((path.clone(), msg.clone()));
                false // remove from cache so hot-reload can retry
            } else {
                true
            }
        });
        failures
    }

    /// Wait for all tasks to complete.
    pub fn wait_all(&self) {
        while !self.queue.lock().is_empty() || !self.pending.is_empty() || !self.active.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn background_worker_limit(total_workers: usize) -> usize {
        if total_workers > 1 {
            total_workers - 1
        } else {
            1
        }
    }
}

// =============================================================================
// Request handling (called by compile())
// =============================================================================

impl CompileScheduler {
    fn reusable_cached_result(result: CompileResult) -> Option<CompileResult> {
        match result {
            CompileResult::Success(_) => Some(result),
            CompileResult::Failed(_) | CompileResult::Skipped => None,
        }
    }

    fn try_join_active(&self, path: &Path, tx: Waiter) -> bool {
        if let Some(mut waiters) = self.active.get_mut(path) {
            waiters.push(tx);
            true
        } else {
            false
        }
    }

    /// Returns true if task needs to be enqueued.
    fn join_or_create_pending(
        &self,
        path: &Path,
        priority: Priority,
        config: Arc<SiteConfig>,
        state: Arc<SiteIndex>,
        tx: Waiter,
    ) -> bool {
        match self.pending.entry(path.to_path_buf()) {
            Entry::Occupied(mut e) => {
                let pending = e.get_mut();
                pending.waiters.push(tx);
                if priority > pending.priority {
                    pending.priority = priority;
                    pending.config = config;
                    pending.state = state;
                    true // upgrade: enqueue higher priority task
                } else {
                    false
                }
            }
            Entry::Vacant(e) => {
                e.insert(PendingState {
                    priority,
                    config,
                    state,
                    waiters: vec![tx],
                });
                true // new task
            }
        }
    }

    fn enqueue(&self, path: PathBuf, priority: Priority) {
        let mut queue = self.queue.lock();
        let task = Task { path, priority };
        if priority == Priority::Background {
            queue.background.push(task);
        } else {
            queue.foreground.push(task);
        }
        self.notify.notify_one();
    }

    fn is_known(&self, path: &Path) -> bool {
        self.cache.contains_key(path)
            || self.active.contains_key(path)
            || self.pending.contains_key(path)
    }

    fn recv(rx: channel::Receiver<CompileResult>) -> CompileResult {
        rx.recv()
            .unwrap_or(CompileResult::Failed("channel closed".into()))
    }
}

// =============================================================================
// Worker
// =============================================================================

impl CompileScheduler {
    fn run_worker(&self) {
        while !self.is_shutdown() {
            if let Some((task, waiters)) = self.next_task() {
                self.execute(task, waiters);
            }
        }
    }

    fn is_shutdown(&self) -> bool {
        self.shutdown.load(AtomicOrdering::SeqCst)
    }

    fn next_task(&self) -> Option<(CompileJob, Vec<Waiter>)> {
        let task = self.dequeue()?;
        let is_background = task.priority == Priority::Background;
        let claimed = self.claim(task);
        if claimed.is_some() && is_background {
            self.queue.lock().active_background += 1;
        }
        claimed
    }

    fn dequeue(&self) -> Option<Task> {
        let mut queue = self.queue.lock();
        while !queue.has_ready_task(self.background_limit()) {
            if self.is_shutdown() {
                return None;
            }
            self.notify.wait(&mut queue);
        }

        if let Some(task) = queue.foreground.pop() {
            return Some(task);
        }

        if queue.active_background < self.background_limit() {
            return queue.background.pop();
        }

        None
    }

    fn claim(&self, task: Task) -> Option<(CompileJob, Vec<Waiter>)> {
        // Atomically claim from pending
        let pending = match self.pending.entry(task.path.clone()) {
            Entry::Occupied(e) => {
                if e.get().priority > task.priority {
                    return None; // stale: higher priority task in queue
                }
                e.remove()
            }
            Entry::Vacant(_) => return None, // already claimed
        };
        let job = CompileJob {
            path: task.path,
            priority: pending.priority,
            config: pending.config,
            state: pending.state,
        };
        let waiters = pending.waiters;

        // Already cached? Notify immediately
        if let Some(result) = self
            .get_cached(&job.path)
            .and_then(Self::reusable_cached_result)
        {
            Self::broadcast(&waiters, result);
            return None;
        }

        // Already active? Merge waiters
        if let Some(mut existing) = self.active.get_mut(&job.path) {
            existing.extend(waiters);
            return None;
        }

        Some((job, waiters))
    }

    fn execute(&self, job: CompileJob, waiters: Vec<Waiter>) {
        let path = job.path.clone();
        self.active.insert(path.clone(), waiters);

        // Catch panics to ensure waiters always receive a result
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.do_compile(&path, &job.config, &job.state)
        }))
        .unwrap_or_else(|_| CompileResult::Failed("compilation panicked".into()));

        let waiters = self
            .active
            .remove(&path)
            .map(|(_, w)| w)
            .unwrap_or_default();

        if matches!(result, CompileResult::Success(_)) {
            self.cache.insert(path, result.clone());
        } else {
            self.cache.remove(&path);
        }
        if job.priority == Priority::Background {
            let mut queue = self.queue.lock();
            queue.active_background = queue.active_background.saturating_sub(1);
            drop(queue);
            self.notify.notify_all();
        }
        Self::broadcast(&waiters, result);
    }

    fn broadcast(waiters: &[Waiter], result: CompileResult) {
        for tx in waiters {
            let _ = tx.send(result.clone());
        }
    }
}

// =============================================================================
// Compilation logic
// =============================================================================

impl CompileScheduler {
    fn do_compile(&self, path: &Path, config: &SiteConfig, state: &SiteIndex) -> CompileResult {
        use crate::compiler::dependency::flush_current_thread_deps;
        use crate::compiler::page::{cache_vdom, process_page, write_page_html};

        let result = match process_page(BuildMode::DEVELOPMENT, path, config, state) {
            Ok(Some(r)) => r,
            Ok(None) => return CompileResult::Skipped,
            Err(e) => return CompileResult::Failed(format!("{:#}", e)),
        };

        // Flush dependencies recorded by process_page to global graph
        // (scheduler workers are not rayon threads, so flush_to_global won't reach them)
        flush_current_thread_deps();

        if let Err(e) = write_page_html(&result.page) {
            return CompileResult::Failed(format!("write failed: {:#}", e));
        }

        if let Some(vdom) = result.indexed_vdom {
            cache_vdom(&result.permalink, vdom);
        }

        state.address().write().update_page(
            result.page.route.clone(),
            result
                .page
                .content_meta
                .as_ref()
                .and_then(|m| m.title.clone()),
        );

        CompileResult::Success(result.page.route.output_file)
    }
}

impl Default for CompileScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueState {
    fn is_empty(&self) -> bool {
        self.foreground.is_empty() && self.background.is_empty()
    }

    fn has_ready_task(&self, background_limit: usize) -> bool {
        !self.foreground.is_empty()
            || (!self.background.is_empty() && self.active_background < background_limit)
    }
}

impl CompileScheduler {
    fn background_limit(&self) -> usize {
        self.background_limit.load(AtomicOrdering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SiteConfig;
    use crate::freshness::compute_file_hash;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn reusable_cached_result_only_accepts_success() {
        let success = CompileResult::Success(PathBuf::from("/tmp/out.html"));
        let failed = CompileResult::Failed("boom".into());
        let skipped = CompileResult::Skipped;

        let reused = CompileScheduler::reusable_cached_result(success);
        assert!(
            matches!(reused, Some(CompileResult::Success(ref path)) if path == &PathBuf::from("/tmp/out.html"))
        );
        assert!(CompileScheduler::reusable_cached_result(failed).is_none());
        assert!(CompileScheduler::reusable_cached_result(skipped).is_none());
    }

    #[test]
    fn invalidate_clears_compile_and_freshness_cache() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("page.typ");
        fs::write(&path, "= Old").unwrap();

        let old_hash = compute_file_hash(&path);
        fs::write(&path, "= New").unwrap();

        SCHEDULER.cache.insert(
            path.clone(),
            CompileResult::Success(PathBuf::from("/tmp/out.html")),
        );
        assert!(SCHEDULER.get_cached(&path).is_some());

        SCHEDULER.invalidate(&path);

        assert!(SCHEDULER.get_cached(&path).is_none());
        assert_ne!(compute_file_hash(&path), old_hash);
    }

    #[test]
    fn background_limit_reserves_one_slot_when_possible() {
        assert_eq!(CompileScheduler::background_worker_limit(1), 1);
        assert_eq!(CompileScheduler::background_worker_limit(2), 1);
        assert_eq!(CompileScheduler::background_worker_limit(8), 7);
    }

    #[test]
    fn priority_upgrade_claims_upgrading_config() {
        fn config_with_root(root: &Path) -> Arc<SiteConfig> {
            let mut config = SiteConfig::default();
            config.set_root(root);
            Arc::new(config)
        }

        let scheduler = CompileScheduler::new();
        let dir = TempDir::new().unwrap();
        let background_root = dir.path().join("background-root");
        let active_root = dir.path().join("active-root");
        let background_config = config_with_root(&background_root);
        let active_config = config_with_root(&active_root);
        let background_state = Arc::new(SiteIndex::new());
        let active_state = Arc::new(SiteIndex::new());
        let path = dir.path().join("content/page.typ");

        let (background_tx, _background_rx) = channel::bounded(1);
        assert!(scheduler.join_or_create_pending(
            &path,
            Priority::Background,
            Arc::clone(&background_config),
            Arc::clone(&background_state),
            background_tx,
        ));

        let (active_tx, _active_rx) = channel::bounded(1);
        assert!(scheduler.join_or_create_pending(
            &path,
            Priority::Active,
            Arc::clone(&active_config),
            Arc::clone(&active_state),
            active_tx,
        ));

        let (job, waiters) = scheduler
            .claim(Task {
                path: path.clone(),
                priority: Priority::Active,
            })
            .expect("active task should claim upgraded pending work");

        assert_eq!(job.path, path);
        assert_eq!(job.priority, Priority::Active);
        assert_eq!(job.config.get_root(), active_root);
        assert!(Arc::ptr_eq(&job.state, &active_state));
        assert_eq!(waiters.len(), 2);
    }
}

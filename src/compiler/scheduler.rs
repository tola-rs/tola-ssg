//! Compile scheduler with priority queue.
//!
//! Single entry point for all compilation requests:
//! - On-demand (user request) → Active priority
//! - Hot-reload → Direct/Affected priority
//! - Background build → Background priority

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::LazyLock;

use crossbeam::channel::{self, Sender};
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use parking_lot::{Condvar, Mutex};

use crate::core::{BuildMode, Priority};

// =============================================================================
// Public API
// =============================================================================

/// Global scheduler instance.
pub static SCHEDULER: LazyLock<CompileScheduler> = LazyLock::new(CompileScheduler::new);

/// Result of a compilation.
#[derive(Debug, Clone)]
pub enum CompileResult {
    Success(PathBuf),
    Failed(String),
    Skipped,
}

// =============================================================================
// Scheduler
// =============================================================================

/// Central compile scheduler with priority queue and deduplication.
pub struct CompileScheduler {
    /// Priority queue of pending tasks
    queue: Mutex<BinaryHeap<Task>>,
    /// Pending paths → waiters (for dedup and result broadcasting)
    pending: DashMap<PathBuf, PendingState>,
    /// In-progress paths → waiters
    active: DashMap<PathBuf, Vec<Waiter>>,
    /// Completed paths → cached results
    cache: DashMap<PathBuf, CompileResult>,
    /// Worker notification
    notify: Condvar,
    /// Shutdown flag
    shutdown: AtomicBool,
    /// Workers started flag
    started: AtomicBool,
}

type Waiter = Sender<CompileResult>;

struct Task {
    path: PathBuf,
    priority: Priority,
}

struct PendingState {
    priority: Priority,
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
            queue: Mutex::new(BinaryHeap::new()),
            pending: DashMap::new(),
            active: DashMap::new(),
            cache: DashMap::new(),
            notify: Condvar::new(),
            shutdown: AtomicBool::new(false),
            started: AtomicBool::new(false),
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
        for _ in 0..n {
            std::thread::spawn(|| SCHEDULER.run_worker());
        }
    }

    /// Request compilation, wait for result.
    pub fn compile(&self, path: PathBuf, priority: Priority) -> CompileResult {
        // Fast path: cached
        if let Some(result) = self.get_cached(&path) {
            return result;
        }

        let (tx, rx) = channel::bounded(1);

        // Try join existing work
        if self.try_join_active(&path, tx.clone()) {
            return Self::recv(rx);
        }

        // Atomically join or create pending
        if self.join_or_create_pending(&path, priority, tx) {
            self.enqueue(path, priority);
        }

        Self::recv(rx)
    }

    /// Submit paths for background compilation (fire-and-forget).
    pub fn submit_background(&self, paths: Vec<PathBuf>) {
        let mut queue = self.queue.lock();
        for path in paths {
            if self.is_known(&path) {
                continue;
            }
            self.pending.insert(path.clone(), PendingState {
                priority: Priority::Background,
                waiters: vec![],
            });
            queue.push(Task { path, priority: Priority::Background });
        }
        self.notify.notify_all();
    }

    /// Invalidate cache (on file change).
    pub fn invalidate(&self, path: &Path) {
        self.cache.remove(path);
    }

    /// Check if compiled.
    pub fn is_compiled(&self, path: &Path) -> bool {
        self.cache.contains_key(path)
    }

    /// Get cached result.
    pub fn get_cached(&self, path: &Path) -> Option<CompileResult> {
        self.cache.get(path).map(|r| r.clone())
    }

    /// Signal shutdown.
    pub fn shutdown(&self) {
        self.shutdown.store(true, AtomicOrdering::SeqCst);
        self.notify.notify_all();
    }

    /// Wait for all tasks to complete.
    pub fn wait_all(&self) {
        while !self.queue.lock().is_empty() || !self.pending.is_empty() || !self.active.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

// =============================================================================
// Request handling (called by compile())
// =============================================================================

impl CompileScheduler {
    fn try_join_active(&self, path: &Path, tx: Waiter) -> bool {
        if let Some(mut waiters) = self.active.get_mut(path) {
            waiters.push(tx);
            true
        } else {
            false
        }
    }

    /// Returns true if task needs to be enqueued.
    fn join_or_create_pending(&self, path: &Path, priority: Priority, tx: Waiter) -> bool {
        match self.pending.entry(path.to_path_buf()) {
            Entry::Occupied(mut e) => {
                let state = e.get_mut();
                state.waiters.push(tx);
                if priority > state.priority {
                    state.priority = priority;
                    true // upgrade: enqueue higher priority task
                } else {
                    false
                }
            }
            Entry::Vacant(e) => {
                e.insert(PendingState { priority, waiters: vec![tx] });
                true // new task
            }
        }
    }

    fn enqueue(&self, path: PathBuf, priority: Priority) {
        self.queue.lock().push(Task { path, priority });
        self.notify.notify_one();
    }

    fn is_known(&self, path: &Path) -> bool {
        self.cache.contains_key(path)
            || self.active.contains_key(path)
            || self.pending.contains_key(path)
    }

    fn recv(rx: channel::Receiver<CompileResult>) -> CompileResult {
        rx.recv().unwrap_or(CompileResult::Failed("channel closed".into()))
    }
}

// =============================================================================
// Worker
// =============================================================================

impl CompileScheduler {
    fn run_worker(&self) {
        while !self.is_shutdown() {
            if let Some((path, waiters)) = self.next_task() {
                self.execute(path, waiters);
            }
        }
    }

    fn is_shutdown(&self) -> bool {
        self.shutdown.load(AtomicOrdering::SeqCst)
    }

    fn next_task(&self) -> Option<(PathBuf, Vec<Waiter>)> {
        let task = self.dequeue()?;
        self.claim(task)
    }

    fn dequeue(&self) -> Option<Task> {
        let mut queue = self.queue.lock();
        while queue.is_empty() {
            if self.is_shutdown() {
                return None;
            }
            self.notify.wait(&mut queue);
        }
        queue.pop()
    }

    fn claim(&self, task: Task) -> Option<(PathBuf, Vec<Waiter>)> {
        // Atomically claim from pending
        let waiters = match self.pending.entry(task.path.clone()) {
            Entry::Occupied(e) => {
                if e.get().priority > task.priority {
                    return None; // stale: higher priority task in queue
                }
                e.remove().waiters
            }
            Entry::Vacant(_) => return None, // already claimed
        };

        // Already cached? Notify immediately
        if let Some(result) = self.get_cached(&task.path) {
            Self::broadcast(&waiters, result);
            return None;
        }

        // Already active? Merge waiters
        if let Some(mut existing) = self.active.get_mut(&task.path) {
            existing.extend(waiters);
            return None;
        }

        Some((task.path, waiters))
    }

    fn execute(&self, path: PathBuf, waiters: Vec<Waiter>) {
        self.active.insert(path.clone(), waiters);

        // Catch panics to ensure waiters always receive a result
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.do_compile(&path)
        }))
        .unwrap_or_else(|_| CompileResult::Failed("compilation panicked".into()));

        let waiters = self.active.remove(&path)
            .map(|(_, w)| w)
            .unwrap_or_default();

        self.cache.insert(path, result.clone());
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
    fn do_compile(&self, path: &Path) -> CompileResult {
        use crate::compiler::page::{cache_vdom, process_page, write_page_html};
        use crate::config::cfg;

        let config = cfg();

        let result = match process_page(BuildMode::DEVELOPMENT, path, &config) {
            Ok(Some(r)) => r,
            Ok(None) => return CompileResult::Skipped,
            Err(e) => return CompileResult::Failed(format!("{:#}", e)),
        };

        if let Err(e) = write_page_html(&result.page) {
            return CompileResult::Failed(format!("write failed: {:#}", e));
        }

        if let Some(vdom) = result.indexed_vdom {
            cache_vdom(&result.permalink, vdom);
        }

        if let Some(mut space) = crate::core::GLOBAL_ADDRESS_SPACE.try_write() {
            space.update_page(
                result.page.route.clone(),
                result.page.content_meta.as_ref().and_then(|m| m.title.clone()),
            );
        }

        CompileResult::Success(result.page.route.output_file)
    }
}

impl Default for CompileScheduler {
    fn default() -> Self {
        Self::new()
    }
}

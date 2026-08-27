//! Bounded cancellation, mailbox, and tracked-thread ownership primitives.

use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

/// Maximum UTF-8 bytes retained in one diagnostic task name.
pub const MAX_TASK_NAME_BYTES: usize = 128;

#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    parent: Option<Arc<CancellationState>>,
}

/// Cloneable hierarchical cancellation observation and subtree control.
#[derive(Clone, Debug)]
pub struct CancellationToken(Arc<CancellationState>);

impl CancellationToken {
    /// Constructs one uncancelled root token.
    pub fn new() -> Self {
        Self(Arc::new(CancellationState {
            cancelled: AtomicBool::new(false),
            parent: None,
        }))
    }

    /// Creates a child that observes this token without controlling its siblings.
    #[must_use]
    pub fn child(&self) -> Self {
        Self(Arc::new(CancellationState {
            cancelled: AtomicBool::new(false),
            parent: Some(Arc::clone(&self.0)),
        }))
    }

    /// Cancels this token and every descendant that observes it.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }

    /// Reports local or ancestor cancellation.
    pub fn is_cancelled(&self) -> bool {
        let mut current = Some(Arc::clone(&self.0));
        while let Some(state) = current {
            if state.cancelled.load(Ordering::Acquire) {
                return true;
            }
            current = state.parent.as_ref().map(Arc::clone);
        }
        false
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct MailboxState<T> {
    queue: VecDeque<T>,
    capacity: usize,
    closed: bool,
}

/// Nonblocking bounded mailbox producer.
#[derive(Debug)]
pub struct MailboxSender<T>(Arc<Mutex<MailboxState<T>>>);

impl<T> Clone for MailboxSender<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

/// Sole bounded mailbox consumer and close owner.
#[derive(Debug)]
pub struct MailboxReceiver<T>(Arc<Mutex<MailboxState<T>>>);

/// Explicit nonblocking send rejection retaining the unsent value.
#[derive(Debug, Eq, PartialEq)]
pub enum MailboxSendError<T> {
    /// The fixed queue capacity is occupied.
    Full(T),
    /// Intake has been closed.
    Closed(T),
}

/// Constructs one fixed-capacity in-memory mailbox.
pub fn bounded_mailbox<T>(capacity: NonZeroUsize) -> (MailboxSender<T>, MailboxReceiver<T>) {
    let state = Arc::new(Mutex::new(MailboxState {
        queue: VecDeque::with_capacity(capacity.get()),
        capacity: capacity.get(),
        closed: false,
    }));
    (MailboxSender(Arc::clone(&state)), MailboxReceiver(state))
}

impl<T> MailboxSender<T> {
    /// Attempts one nonblocking bounded send.
    pub fn try_send(&self, value: T) -> Result<(), MailboxSendError<T>> {
        let mut state = lock(&self.0);
        if state.closed {
            return Err(MailboxSendError::Closed(value));
        }
        if state.queue.len() == state.capacity {
            return Err(MailboxSendError::Full(value));
        }
        state.queue.push_back(value);
        Ok(())
    }
}

impl<T> MailboxReceiver<T> {
    /// Removes the oldest item without blocking.
    pub fn try_receive(&self) -> Result<T, MailboxReceiveError> {
        let mut state = lock(&self.0);
        if let Some(value) = state.queue.pop_front() {
            return Ok(value);
        }
        Err(if state.closed {
            MailboxReceiveError::Closed
        } else {
            MailboxReceiveError::Empty
        })
    }

    /// Closes producer intake without discarding accepted items.
    pub fn close(&self) {
        lock(&self.0).closed = true;
    }
}

impl<T> Drop for MailboxReceiver<T> {
    fn drop(&mut self) {
        self.close();
    }
}

/// Explicit nonblocking receive state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailboxReceiveError {
    /// No item is currently available but producer intake remains open.
    Empty,
    /// Producer intake is closed and every accepted item has been drained.
    Closed,
}

/// Stable tracked task result failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskError;

impl TaskError {
    /// Constructs a redacted task failure.
    pub const fn failed() -> Self {
        Self
    }
}

/// Task intake rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskTrackerError {
    /// Every fixed task slot is occupied.
    Full,
    /// Spawn intake has closed for drain.
    Closed,
    /// The operating system could not start the named thread.
    SpawnFailed,
    /// The diagnostic name was empty or exceeded its fixed bound.
    InvalidName,
}

/// Stable joined task failure class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskFailureKind {
    /// The task returned an explicit failure.
    Failed,
    /// The task panicked.
    Panicked,
}

/// One retained named task failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskFailure {
    /// Bounded caller-supplied diagnostic task name.
    pub name: String,
    /// Whether the task failed explicitly or panicked.
    pub kind: TaskFailureKind,
}

/// Complete join report for every accepted task.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskJoinReport {
    /// Number of joined accepted tasks.
    pub joined: usize,
    /// Stable named failures.
    pub failures: Vec<TaskFailure>,
}

/// Fixed-capacity owner of every accepted thread handle.
#[derive(Debug)]
pub struct TaskTracker {
    capacity: usize,
    open: bool,
    tasks: Vec<(String, JoinHandle<Result<(), TaskError>>)>,
}

impl TaskTracker {
    /// Constructs an open tracker with fixed live-task capacity.
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            capacity: capacity.get(),
            open: true,
            tasks: Vec::with_capacity(capacity.get()),
        }
    }

    /// Reserves capacity, starts, and retains one named task handle.
    pub fn spawn(
        &mut self,
        name: impl Into<String>,
        task: impl FnOnce() -> Result<(), TaskError> + Send + 'static,
    ) -> Result<(), TaskTrackerError> {
        if !self.open {
            return Err(TaskTrackerError::Closed);
        }
        if self.tasks.len() == self.capacity {
            return Err(TaskTrackerError::Full);
        }
        let name = name.into();
        if name.is_empty() || name.len() > MAX_TASK_NAME_BYTES {
            return Err(TaskTrackerError::InvalidName);
        }
        let handle = thread::Builder::new()
            .name(name.clone())
            .spawn(task)
            .map_err(|_| TaskTrackerError::SpawnFailed)?;
        self.tasks.push((name, handle));
        Ok(())
    }

    /// Closes future spawn intake idempotently.
    pub fn close_intake(&mut self) {
        self.open = false;
    }

    /// Joins every accepted handle and empties the tracker.
    pub fn join_all(&mut self) -> TaskJoinReport {
        self.close_intake();
        let mut report = TaskJoinReport::default();
        for (name, task) in self.tasks.drain(..) {
            report.joined += 1;
            match task.join() {
                Ok(Ok(())) => {}
                Ok(Err(_)) => report.failures.push(TaskFailure {
                    name,
                    kind: TaskFailureKind::Failed,
                }),
                Err(_) => report.failures.push(TaskFailure {
                    name,
                    kind: TaskFailureKind::Panicked,
                }),
            }
        }
        report
    }

    /// Returns the number of retained live or completed-but-unjoined handles.
    pub fn live_count(&self) -> usize {
        self.tasks.len()
    }
}

impl Drop for TaskTracker {
    fn drop(&mut self) {
        let _ = self.join_all();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

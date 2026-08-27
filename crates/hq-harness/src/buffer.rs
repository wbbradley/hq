//! Fixed-capacity durable FIFO and replaceable-snapshot buffering.

use std::{collections::VecDeque, num::NonZeroUsize};

use hq_domain::{CommandDigest, MessageId, OperationId, ShortText};

use crate::{HarnessActivity, HarnessOutput};

/// Stable operation-scoped identity for one replaceable activity snapshot.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HarnessSnapshotKey {
    /// Operation whose presentation is being replaced.
    pub operation_id: OperationId,
    /// Stable neutral logical key within the operation.
    pub logical_key: ShortText,
}

/// One accepted persistence item or replaceable pre-persistence snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessBufferedEvent {
    /// Durable output that must persist before any later buffered activity.
    Output {
        /// Stable event checkpoint identity.
        event_id: MessageId,
        /// Digest of the complete normalized output under that identity.
        digest: CommandDigest,
        /// Typed bounded output.
        output: HarnessOutput,
    },
    /// Durable activity that must retain FIFO order.
    Activity {
        /// Stable event checkpoint identity.
        event_id: MessageId,
        /// Digest of the complete normalized activity under that identity.
        digest: CommandDigest,
        /// Typed bounded activity.
        activity: HarnessActivity,
    },
    /// One output/activity pair whose output always persists first.
    OutputAndActivity {
        /// Stable event checkpoint identity shared by the pair.
        event_id: MessageId,
        /// Digest of the complete normalized pair under that identity.
        digest: CommandDigest,
        /// Typed bounded output persisted first.
        output: HarnessOutput,
        /// Typed bounded activity persisted second.
        activity: HarnessActivity,
    },
    /// Replaceable activity that may coalesce only before persistence accepts it.
    Snapshot {
        /// Stable event checkpoint identity for the accepted newest value.
        event_id: MessageId,
        /// Digest of the complete normalized newest value.
        digest: CommandDigest,
        /// Exact coalescing identity.
        key: HarnessSnapshotKey,
        /// Newest typed bounded value.
        activity: HarnessActivity,
    },
}

/// Successful bounded-buffer admission class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HarnessBufferPush {
    /// A new durable item or snapshot key entered the tail.
    Accepted,
    /// An older snapshot with the same exact key was removed and this value entered the tail.
    Replaced,
}

/// Sole mutable owner of fixed-capacity normalized persistence work.
pub struct HarnessEventBuffer {
    capacity: NonZeroUsize,
    events: VecDeque<HarnessBufferedEvent>,
}

impl HarnessEventBuffer {
    /// Creates an empty buffer with one explicit nonzero pending-item bound.
    pub const fn new(capacity: NonZeroUsize) -> Self {
        Self {
            capacity,
            events: VecDeque::new(),
        }
    }

    /// Accepts a new item, coalesces one exact snapshot key, or returns the unaccepted item.
    pub fn push(
        &mut self,
        event: HarnessBufferedEvent,
    ) -> Result<HarnessBufferPush, Box<HarnessBufferedEvent>> {
        if let HarnessBufferedEvent::Snapshot { key, .. } = &event
            && let Some(index) = self.events.iter().position(|pending| {
                matches!(
                    pending,
                    HarnessBufferedEvent::Snapshot {
                        key: pending_key,
                        ..
                    } if pending_key == key
                )
            })
        {
            let _ = self.events.remove(index);
            self.events.push_back(event);
            return Ok(HarnessBufferPush::Replaced);
        }
        if self.events.len() == self.capacity.get() {
            return Err(Box::new(event));
        }
        self.events.push_back(event);
        Ok(HarnessBufferPush::Accepted)
    }

    /// Removes the oldest accepted item.
    pub fn pop(&mut self) -> Option<HarnessBufferedEvent> {
        self.events.pop_front()
    }

    /// Borrows the oldest accepted item without removing its recovery obligation.
    pub fn front(&self) -> Option<&HarnessBufferedEvent> {
        self.events.front()
    }

    /// Returns the number of accepted pending items.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Reports whether no accepted item remains.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl std::fmt::Debug for HarnessEventBuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HarnessEventBuffer")
            .field("capacity", &self.capacity)
            .field("pending", &self.events.len())
            .finish()
    }
}

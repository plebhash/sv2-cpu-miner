pub mod extended;
pub mod standard;

/// A poisoned channel lock means a task panicked while updating channel state, which is a
/// bug; mining cannot sensibly continue on that channel.
pub(crate) const LOCK_POISONED: &str = "channel lock poisoned by a panicking task";

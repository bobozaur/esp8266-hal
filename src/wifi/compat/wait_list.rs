//! Compact task IDs for a queue's bounded set of blocked senders or receivers.

const MAX_WAITERS: usize = 8;
const EMPTY: u8 = u8::MAX;

pub(super) struct WaitList {
    entries: [u8; MAX_WAITERS],
}

impl WaitList {
    pub(super) const fn new() -> Self {
        Self {
            entries: [EMPTY; MAX_WAITERS],
        }
    }

    #[inline(never)]
    pub(super) fn add(&mut self, task_idx: usize) {
        debug_assert!(task_idx < usize::from(EMPTY));
        let task_idx = task_idx as u8;
        let mut free_slot = None;
        for (slot, &entry) in self.entries.iter().enumerate() {
            if entry == task_idx {
                return;
            }
            if entry == EMPTY && free_slot.is_none() {
                free_slot = Some(slot);
            }
        }
        if let Some(slot) = free_slot {
            self.entries[slot] = task_idx;
        }
    }

    #[inline(never)]
    pub(super) fn remove(&mut self, task_idx: usize) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| usize::from(**entry) == task_idx)
        {
            *entry = EMPTY;
        }
    }

    /// Equal priorities use the first occupied slot, including holes reused by
    /// later additions. This preserves the existing queue order rather than FIFO.
    pub(super) fn take_highest_priority(
        &mut self,
        mut priority: impl FnMut(usize) -> u32,
    ) -> Option<usize> {
        let mut best = None;
        let mut best_priority = 0;
        for (slot, &entry) in self.entries.iter().enumerate() {
            if entry == EMPTY {
                continue;
            }
            let task_priority = priority(usize::from(entry));
            if best.is_none() || task_priority > best_priority {
                best = Some(slot);
                best_priority = task_priority;
            }
        }
        best.map(|slot| {
            let task_idx = self.entries[slot];
            self.entries[slot] = EMPTY;
            usize::from(task_idx)
        })
    }

    pub(super) fn has_waiter(&self) -> bool {
        self.entries.iter().any(|&entry| entry != EMPTY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_waiters_bound_capacity_and_ignore_duplicate_registration() {
        assert_eq!(core::mem::size_of::<WaitList>(), 8);
        let mut list = WaitList::new();
        assert!(!list.has_waiter());
        list.add(0);
        list.add(0);
        for task in 1..8 {
            list.add(task);
        }
        list.add(9);
        for expected in 0..8 {
            assert_eq!(list.take_highest_priority(|_| 0), Some(expected));
        }
        assert_eq!(list.take_highest_priority(|_| 0), None);
        assert!(!list.has_waiter());
        list.add(9);
        assert_eq!(list.take_highest_priority(|_| 0), Some(9));
    }

    #[test]
    fn priority_selection_and_cancellation_preserve_first_occupied_slot_order() {
        let mut list = WaitList::new();
        for task in [2, 4, 6] {
            list.add(task);
        }
        list.remove(2);
        list.remove(2);
        list.add(8);
        assert_eq!(
            list.take_highest_priority(|task| if task == 6 { 5 } else { 1 }),
            Some(6)
        );
        assert_eq!(list.take_highest_priority(|_| 1), Some(8));
        assert_eq!(list.take_highest_priority(|_| 1), Some(4));
        assert!(!list.has_waiter());
        // A deleted/cancelled task ID can be reused without an old waiter.
        list.add(4);
        list.remove(4);
        list.add(4);
        assert_eq!(list.take_highest_priority(|_| 0), Some(4));
        assert_eq!(list.take_highest_priority(|_| 0), None);
    }
}

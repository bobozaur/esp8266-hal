use super::{allocator::RuntimeAllocator, InitError};
use allocator_api2::vec::Vec;

/// Fixed admission bound; initialization is the only allocation point.
pub(crate) struct Queue<T> {
    entries: Vec<Option<T>, RuntimeAllocator>,
    head: usize,
    len: usize,
}

impl<T> Queue<T> {
    pub(crate) fn try_new(capacity: usize, allocator: RuntimeAllocator) -> Result<Self, InitError> {
        let mut entries = Vec::new_in(allocator);
        entries
            .try_reserve_exact(capacity)
            .map_err(|_| InitError::OutOfMemory)?;
        entries.resize_with(capacity, || None);
        Ok(Self {
            entries,
            head: 0,
            len: 0,
        })
    }

    pub(crate) fn capacity(&self) -> usize {
        self.entries.len()
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn try_push(&mut self, item: T) -> Result<(), T> {
        if self.len == self.capacity() {
            return Err(item);
        }
        let tail = (self.head + self.len) % self.capacity();
        self.entries[tail] = Some(item);
        self.len += 1;
        Ok(())
    }

    pub(crate) fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let item = self.entries[self.head].take();
        self.head = (self.head + 1) % self.capacity();
        self.len -= 1;
        item
    }

    #[cfg(any(
        feature = "wifi-esp-now",
        feature = "wifi-promiscuous",
        feature = "wifi-wps",
        feature = "wifi-smartconfig"
    ))]
    pub(crate) fn clear(&mut self) {
        while self.pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saturation_zero_capacity_and_wraparound_preserve_order() {
        let allocator = super::super::allocator::test_allocator();
        let mut empty = Queue::try_new(0, allocator).unwrap();
        assert_eq!(empty.try_push(4), Err(4));
        assert_eq!(empty.pop(), None);
        let mut queue = Queue::try_new(2, allocator).unwrap();
        queue.try_push(1).unwrap();
        queue.try_push(2).unwrap();
        assert_eq!(queue.try_push(3), Err(3));
        assert_eq!(queue.pop(), Some(1));
        queue.try_push(3).unwrap();
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.capacity(), 2);
    }
}

//! Inline storage for shallow containers, with safe spillover.

use alloc::vec::Vec;

pub(crate) struct Stack<T: Copy> {
    inline: [Option<T>; 16],
    len: usize,
    spill: Vec<T>,
}

impl<T: Copy> Stack<T> {
    pub(crate) fn new() -> Self {
        Self {
            inline: [None; 16],
            len: 0,
            spill: Vec::new(),
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub(crate) fn push(&mut self, value: T) {
        if self.len < self.inline.len() {
            self.inline[self.len] = Some(value);
        } else {
            self.spill.push(value);
        }
        self.len += 1;
    }

    #[inline]
    pub(crate) fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        if self.len < self.inline.len() {
            self.inline[self.len].take()
        } else {
            self.spill.pop()
        }
    }

    #[inline]
    pub(crate) fn last(&self) -> Option<&T> {
        if self.len > self.inline.len() {
            self.spill.last()
        } else {
            self.inline
                .get(self.len.checked_sub(1)?)
                .and_then(Option::as_ref)
        }
    }

    #[inline]
    pub(crate) fn last_mut(&mut self) -> Option<&mut T> {
        if self.len > self.inline.len() {
            self.spill.last_mut()
        } else {
            self.inline
                .get_mut(self.len.checked_sub(1)?)
                .and_then(Option::as_mut)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Stack;

    #[test]
    fn spill_and_reuse_preserve_stack_order() {
        let mut stack = Stack::new();
        for _ in 0..3 {
            for value in 0..128 {
                stack.push(value);
            }
            for expected in (0..128).rev() {
                assert_eq!(stack.last(), Some(&expected));
                *stack.last_mut().unwrap() += 1;
                assert_eq!(stack.pop(), Some(expected + 1));
            }
            assert!(stack.is_empty());
            assert_eq!(stack.pop(), None);
            assert_eq!(stack.last_mut(), None);
        }
    }
}

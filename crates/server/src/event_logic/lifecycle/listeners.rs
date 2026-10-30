// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Native scalar observer vectors: 04055e00/04055ec0/04055f70 and 04015630.
//! A nested dispatch clears the same Boolean iteration flag. Null slots are
//! removed by swapping the tail, and a listener appended in a callback is live.
#[derive(Clone, Debug)]
pub(super) struct List<T> {
    pub items: Vec<Option<T>>,
    removed: usize,
    pub notifying: bool,
}
impl<T> Default for List<T> {
    fn default() -> Self {
        Self {
            items: vec![],
            removed: 0,
            notifying: false,
        }
    }
}
impl<T: Copy + Eq> List<T> {
    pub fn remove(&mut self, listener: T) {
        let Some(item) = self.items.iter_mut().find(|item| **item == Some(listener)) else {
            return;
        };
        *item = None;
        self.removed += 1;
        if self.removed > 100 && !self.notifying {
            let mut index = 0;
            while index < self.items.len() {
                if self.items[index].is_none() {
                    self.items.swap_remove(index);
                } else {
                    index += 1;
                }
            }
            self.removed = 0;
        }
    }
    pub fn next(&mut self, index: usize) -> Option<T> {
        while index < self.items.len() {
            if let Some(listener) = self.items[index] {
                return Some(listener);
            }
            self.items.swap_remove(index);
        }
        self.notifying = false;
        None
    }
}

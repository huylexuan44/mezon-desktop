use gpui::Context;

/// Derived view state that calls `cx.notify()` only when it actually changes — the GPUI analog of
/// React-Redux's `useSelector` referential-equality skip.
///
/// A view scoped to a sub-slice of a coarse global store can recompute freely on every store
/// notify and update through `Derived`; the re-render only happens when the rendered output really
/// changed. Replaces the hand-written `if next != self.x { self.x = next; cx.notify() }` guard.
pub struct Derived<T> {
    value: T,
}

impl<T> Derived<T> {
    pub fn new(value: T) -> Self {
        Self { value }
    }

    pub fn get(&self) -> &T {
        &self.value
    }
}

impl<T: PartialEq> Derived<T> {
    /// Replace with `next`, calling `cx.notify()` only if it differs. Returns whether it changed.
    pub fn set<V: 'static>(&mut self, next: T, cx: &mut Context<V>) -> bool {
        if self.value != next {
            self.value = next;
            cx.notify();
            true
        } else {
            false
        }
    }
}

impl<T: Default> Default for Derived<T> {
    fn default() -> Self {
        Self {
            value: T::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_and_default() {
        let derived = Derived::new(vec![1, 2, 3]);
        assert_eq!(derived.get(), &vec![1, 2, 3]);
        let empty: Derived<Vec<i32>> = Derived::default();
        assert!(empty.get().is_empty());
    }
}

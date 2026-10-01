//! A small, bounded, thread-safe memo for highlighting results.
//!
//! syntect accounts for ~98% of the highlighting time (≈0.5 MB/s for Rust, see `eval.rs`
//! `overhead`), and the live server re-renders a whole document on every save while most of its
//! code blocks are unchanged. Results are pure functions of `(code, lang)`, so memoizing them is
//! transparent. The memo is cleared wholesale when it exceeds its entry or byte budget (simple,
//! and the output never depends on its state).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

/// Maximum number of memoized blocks.
const MAX_ENTRIES: usize = 2048;
/// Maximum total bytes of memoized code plus results.
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// Results whose size can be accounted for.
pub(crate) trait Weigh {
    fn weight(&self) -> usize;
}

pub(crate) struct Memo<V> {
    inner: Mutex<Inner<V>>,
}

struct Inner<V> {
    map: HashMap<(String, String), V>,
    bytes: usize,
}

impl<V: Clone + Weigh> Memo<V> {
    pub(crate) fn new() -> Self {
        Memo {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                bytes: 0,
            }),
        }
    }

    /// The memoized value for `(code, lang)`, computing it with `f` on a miss. `f` runs outside
    /// the lock, so concurrent misses may compute the same value twice (harmlessly).
    pub(crate) fn get_or(&self, code: &str, lang: &str, f: impl FnOnce() -> V) -> V {
        let key = (code.to_owned(), lang.to_owned());
        {
            let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(v) = inner.map.get(&key) {
                return v.clone();
            }
        }
        let value = f();
        let size = key.0.len() + key.1.len() + value.weight();
        if size <= MAX_BYTES / 4 {
            let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            if inner.map.len() >= MAX_ENTRIES || inner.bytes + size > MAX_BYTES {
                inner.map.clear();
                inner.bytes = 0;
            }
            if inner.map.insert(key, value.clone()).is_none() {
                inner.bytes += size;
            }
        }
        value
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Weigh for String {
        fn weight(&self) -> usize {
            self.len()
        }
    }

    #[test]
    fn memoizes_and_bounds() {
        let memo: Memo<String> = Memo::new();
        let mut calls = 0;
        for _ in 0..3 {
            let v = memo.get_or("code", "rust", || {
                calls += 1;
                "out".to_owned()
            });
            assert_eq!(v, "out");
        }
        assert_eq!(calls, 1);
        for i in 0..MAX_ENTRIES + 10 {
            memo.get_or(&i.to_string(), "x", String::new);
        }
        assert!(memo.len() <= MAX_ENTRIES);
    }
}

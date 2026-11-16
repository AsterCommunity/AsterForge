//! Exercise the unsafe wrapper traversal without unsupported process spawning.
//!
//! A terminal non-native child is permitted by process-wrap's contract. Test
//! both that terminal and nested transparent wrappers through the real backend.

use super::*;
use crate::tests::ready;

/// A valid wrapper-chain terminus that has no native Tokio child.
#[derive(Debug)]
struct Terminal;

impl ChildWrapper for Terminal {
    fn inner(&self) -> &dyn ChildWrapper {
        self
    }

    fn inner_mut(&mut self) -> &mut dyn ChildWrapper {
        self
    }

    fn into_inner(self: Box<Self>) -> Box<dyn ChildWrapper> {
        self
    }
}

/// A transparent ownership layer that forwards access to its inner wrapper.
#[derive(Debug)]
struct Transparent(Box<dyn ChildWrapper>);

impl ChildWrapper for Transparent {
    fn inner(&self) -> &dyn ChildWrapper {
        &*self.0
    }

    fn inner_mut(&mut self) -> &mut dyn ChildWrapper {
        &mut *self.0
    }

    fn into_inner(self: Box<Self>) -> Box<dyn ChildWrapper> {
        self.0
    }
}

#[test]
fn native_child_lookup_rejects_terminal_and_nested_non_native_wrappers() {
    let mut wrapper: Box<dyn ChildWrapper> = Box::new(Terminal);
    for _ in 0..4 {
        // This executes the backend's unsafe try_inner_child_mut call, then
        // retains and reuses the wrapper to check ownership after traversal.
        let error = ready(backend::wait_direct(&mut *wrapper)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(wrapper.try_inner_child().is_none());
        wrapper = Box::new(Transparent(wrapper));
    }
}

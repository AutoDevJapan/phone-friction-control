use std::sync::atomic::{AtomicI32, Ordering};

/// One native UI owns the process at a time. Stale update tasks must reuse it;
/// they cannot build a second winit loop while the first one is merely paused.
pub(crate) struct UiOwner(AtomicI32);
pub(crate) struct UiLease<'a>(&'a UiOwner);

impl UiOwner {
    pub const fn new() -> Self {
        Self(AtomicI32::new(-1))
    }

    pub fn claim(&self, task: i32) -> Result<UiLease<'_>, i32> {
        self.0
            .compare_exchange(-1, task, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| UiLease(self))
    }
}

impl Drop for UiLease<'_> {
    fn drop(&mut self) {
        self.0.0.store(-1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_task_reuses_live_owner_and_recreation_waits_for_full_release() {
        let owner = UiOwner::new();
        let first = owner.claim(75).ok().unwrap();
        assert!(matches!(owner.claim(72), Err(75)));
        assert!(matches!(owner.claim(75), Err(75)));
        drop(first);
        let recreated = owner.claim(75).ok().unwrap();
        assert!(matches!(owner.claim(72), Err(75)));
        drop(recreated);
        assert!(owner.claim(72).is_ok());
    }
}

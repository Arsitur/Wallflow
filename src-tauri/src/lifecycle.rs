use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Default)]
pub struct Lifecycle {
    epoch: AtomicU64,
    closing: AtomicBool,
    restore: AtomicBool,
    preparing: AtomicBool,
}
impl Lifecycle {
    pub fn is_closing(&self) -> bool { self.closing.load(Ordering::Acquire) }
    pub fn is_preparing(&self) -> bool { self.preparing.load(Ordering::Acquire) }
    pub fn open(&self) -> u64 {
        self.closing.store(false, Ordering::Release);
        self.preparing.store(true, Ordering::Release);
        self.epoch.fetch_add(1, Ordering::AcqRel) + 1
    }
    pub fn close(&self, restore: bool) -> Option<u64> {
        if self.closing.swap(true, Ordering::AcqRel) { return None; }
        self.restore.store(restore, Ordering::Release);
        self.preparing.store(false, Ordering::Release);
        Some(self.epoch.fetch_add(1, Ordering::AcqRel) + 1)
    }
    pub fn present(&self, epoch: u64) -> bool {
        self.epoch.load(Ordering::Acquire) == epoch && !self.is_closing() && self.preparing.swap(false, Ordering::AcqRel)
    }
    pub fn finish(&self, epoch: u64) -> Option<bool> {
        if self.epoch.load(Ordering::Acquire) != epoch || !self.closing.swap(false, Ordering::AcqRel) { return None; }
        Some(self.restore.load(Ordering::Acquire))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_animation_cannot_hide_a_reopened_gallery() {
        let state = Lifecycle::default();
        state.open();
        let old = state.close(true).unwrap();
        state.open();
        assert_eq!(state.finish(old), None);
        let current = state.close(false).unwrap();
        assert_eq!(state.finish(old), None);
        assert_eq!(state.finish(current), Some(false));
        assert_eq!(state.finish(current), None);
    }
    #[test]
    fn blur_during_an_explicit_close_does_not_cancel_focus_restore() {
        let state = Lifecycle::default();
        state.open();
        let close = state.close(true).unwrap();
        assert_eq!(state.close(false), None);
        assert_eq!(state.finish(close), Some(true));
    }
    #[test]
    fn hidden_preparation_needs_the_current_frontend_acknowledgement() {
        let state = Lifecycle::default();
        let old = state.open();
        assert!(state.is_preparing());
        let current = state.open();
        assert!(!state.present(old));
        assert!(state.is_preparing());
        assert!(state.present(current));
        assert!(!state.is_preparing());
        assert!(!state.present(current));
        let cancelled = state.open();
        state.close(false);
        assert!(!state.present(cancelled));
    }
}

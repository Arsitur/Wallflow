use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

// A ready WebView is not an open request. Keep an explicit shortcut/tray request
// made during startup, but never open just because the frontend was loaded.
#[derive(Default)]
pub struct FrontendReadiness(Mutex<(bool, bool)>);
impl FrontendReadiness {
    pub fn request_open(&self) -> bool {
        let mut state = self.0.lock().unwrap();
        if state.0 { true } else { state.1 = true; false }
    }
    pub fn ready(&self) -> bool {
        let mut state = self.0.lock().unwrap();
        state.0 = true;
        std::mem::take(&mut state.1)
    }
    pub fn cancel_pending(&self) -> bool {
        std::mem::take(&mut self.0.lock().unwrap().1)
    }
}

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
    pub fn dismiss(&self) -> u64 {
        // An auxiliary window takes over focus. Invalidate pending animations,
        // including an earlier explicit close that wanted to restore another app.
        self.restore.store(false, Ordering::Release);
        self.preparing.store(false, Ordering::Release);
        self.closing.store(true, Ordering::Release);
        self.epoch.fetch_add(1, Ordering::AcqRel) + 1
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
    fn starting_or_reloading_the_frontend_does_not_request_a_gallery() {
        let state = FrontendReadiness::default();
        assert!(!state.ready());
        assert!(!state.ready());
        assert!(state.request_open());
    }
    #[test]
    fn a_shortcut_before_readiness_is_kept_and_can_be_cancelled() {
        let state = FrontendReadiness::default();
        assert!(!state.request_open());
        assert!(state.ready());
        assert!(!state.ready());
        let state = FrontendReadiness::default();
        assert!(!state.request_open());
        assert!(state.cancel_pending());
        assert!(!state.ready());
    }
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
    #[test]
    fn settings_dismissal_cancels_pending_present_and_previous_focus_restore() {
        let state = Lifecycle::default();
        let opening = state.open();
        let dismissed = state.dismiss();
        assert!(!state.present(opening));
        assert_eq!(state.finish(dismissed), Some(false));
        state.open();
        let closing = state.close(true).unwrap();
        let dismissed = state.dismiss();
        assert_eq!(state.finish(closing), None);
        assert_eq!(state.finish(dismissed), Some(false));
        assert!(!state.is_preparing());
    }
}

//! Configuration shared by every detour. Written by `install`/`update`, read on hot paths,
//! so the parts that matter per call are atomics and the rest sits behind a `RwLock`.

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::RwLock;

use lsa_proto::{ControllerAssignment, HookFlags, InstanceConfig, PixelRect};

pub const F_XINPUT: u32 = 1 << 0;
pub const F_FAKE_FOCUS: u32 = 1 << 1;
pub const F_FORCE_WINDOWED: u32 = 1 << 2;
pub const F_LIE_SCREEN_SIZE: u32 = 1 << 3;
pub const F_PIN_WINDOW: u32 = 1 << 4;
pub const F_CLIP_CURSOR: u32 = 1 << 5;

/// Everything the detours may need that is not on the atomic fast path.
#[derive(Debug, Clone)]
pub struct Shared {
    pub player: u8,
    pub viewport: PixelRect,
    pub flags: HookFlags,
    pub title_regex: Option<regex_lite::Regex>,
}

static SHARED: RwLock<Option<Shared>> = RwLock::new(None);
static FLAGS: AtomicU32 = AtomicU32::new(0);
/// Physical XInput slot assigned to this instance, or -1 for none.
static XINPUT_SLOT: AtomicI32 = AtomicI32::new(-1);
/// Main window handle as an integer, 0 while unknown.
static MAIN_HWND: AtomicUsize = AtomicUsize::new(0);
/// Viewport as four atomics so `GetSystemMetrics` and friends need no lock.
static VP_X: AtomicI32 = AtomicI32::new(0);
static VP_Y: AtomicI32 = AtomicI32::new(0);
static VP_W: AtomicI32 = AtomicI32::new(0);
static VP_H: AtomicI32 = AtomicI32::new(0);

pub fn flag_bits(f: &HookFlags) -> u32 {
    let mut bits = 0;
    if f.xinput {
        bits |= F_XINPUT;
    }
    if f.fake_focus {
        bits |= F_FAKE_FOCUS;
    }
    if f.force_windowed {
        bits |= F_FORCE_WINDOWED;
    }
    if f.lie_screen_size {
        bits |= F_LIE_SCREEN_SIZE;
    }
    if f.pin_window {
        bits |= F_PIN_WINDOW;
    }
    if f.clip_cursor {
        bits |= F_CLIP_CURSOR;
    }
    bits
}

/// Physical slot for an assignment, or `None` when the instance owns no XInput pad.
pub fn xinput_slot_of(assignment: &ControllerAssignment) -> Option<u32> {
    match assignment {
        ControllerAssignment::XInputSlot { slot } if *slot <= 3 => Some(*slot),
        _ => None,
    }
}

pub fn set(config: &InstanceConfig) {
    let title_regex = config
        .window_title_regex
        .as_deref()
        .and_then(|re| regex_lite::Regex::new(re).ok());
    let shared = Shared {
        player: config.player,
        viewport: config.viewport,
        flags: config.hooks.clone(),
        title_regex,
    };
    FLAGS.store(flag_bits(&config.hooks), Ordering::SeqCst);
    XINPUT_SLOT.store(
        xinput_slot_of(&config.controller)
            .map(|s| s as i32)
            .unwrap_or(-1),
        Ordering::SeqCst,
    );
    VP_X.store(config.viewport.x, Ordering::SeqCst);
    VP_Y.store(config.viewport.y, Ordering::SeqCst);
    VP_W.store(config.viewport.width, Ordering::SeqCst);
    VP_H.store(config.viewport.height, Ordering::SeqCst);
    if let Ok(mut guard) = SHARED.write() {
        *guard = Some(shared);
    }
}

pub fn has(flag: u32) -> bool {
    FLAGS.load(Ordering::Relaxed) & flag != 0
}

pub fn xinput_slot() -> Option<u32> {
    let s = XINPUT_SLOT.load(Ordering::Relaxed);
    (s >= 0).then_some(s as u32)
}

pub fn viewport() -> PixelRect {
    PixelRect::new(
        VP_X.load(Ordering::Relaxed),
        VP_Y.load(Ordering::Relaxed),
        VP_W.load(Ordering::Relaxed),
        VP_H.load(Ordering::Relaxed),
    )
}

pub fn main_window() -> usize {
    MAIN_HWND.load(Ordering::Relaxed)
}

pub fn set_main_window(h: usize) {
    MAIN_HWND.store(h, Ordering::SeqCst);
}

/// Run `f` with the shared config, if any was set.
pub fn with<R>(f: impl FnOnce(&Shared) -> R) -> Option<R> {
    let guard = SHARED.read().ok()?;
    guard.as_ref().map(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_bits_follow_the_config() {
        let mut f = HookFlags::default();
        assert_eq!(
            flag_bits(&f),
            F_XINPUT | F_FAKE_FOCUS | F_FORCE_WINDOWED | F_LIE_SCREEN_SIZE | F_PIN_WINDOW
        );
        f.clip_cursor = true;
        f.xinput = false;
        assert_eq!(
            flag_bits(&f),
            F_FAKE_FOCUS | F_FORCE_WINDOWED | F_LIE_SCREEN_SIZE | F_PIN_WINDOW | F_CLIP_CURSOR
        );
    }

    #[test]
    fn xinput_slot_only_for_valid_xinput_assignments() {
        assert_eq!(
            xinput_slot_of(&ControllerAssignment::XInputSlot { slot: 2 }),
            Some(2)
        );
        assert_eq!(
            xinput_slot_of(&ControllerAssignment::XInputSlot { slot: 9 }),
            None
        );
        assert_eq!(xinput_slot_of(&ControllerAssignment::None), None);
        assert_eq!(
            xinput_slot_of(&ControllerAssignment::HidPath { path: "x".into() }),
            None
        );
    }
}

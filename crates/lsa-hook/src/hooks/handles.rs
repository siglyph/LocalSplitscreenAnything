//! Named kernel object rename: `CreateMutexW("Global\\MyGame")` in player 2's instance
//! becomes `CreateMutexW("Global\\MyGame_LSA2")`, so a game's "already running?" check never
//! sees its sibling instances. Applies to the names listed in the profile, or to every name
//! in aggressive mode.

/// Suffix appended to renamed object names, followed by the player number.
pub const SUFFIX: &str = "_LSA";

/// The rename policy for this instance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenameRule {
    pub player: u8,
    /// Object names to rename (compared case-insensitively, with or without their
    /// `Global\`/`Local\`/`\\.\pipe\` prefix).
    pub names: Vec<String>,
    /// Rename every named object the process creates.
    pub all: bool,
}

/// Strip the namespace prefix games and Windows put in front of object names.
fn bare_name(name: &str) -> &str {
    for prefix in ["Global\\", "Local\\", "\\\\.\\pipe\\", "\\\\?\\pipe\\"] {
        if name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return &name[prefix.len()..];
        }
    }
    if let Some(rest) = name.strip_prefix("Session\\") {
        // `Session\<n>\name`
        if let Some(i) = rest.find('\\') {
            return &rest[i + 1..];
        }
    }
    name
}

fn matches(rule_name: &str, name: &str) -> bool {
    rule_name.eq_ignore_ascii_case(name) || bare_name(rule_name).eq_ignore_ascii_case(bare_name(name))
}

/// The new name for `name`, or `None` to leave it alone.
pub fn renamed(rule: &RenameRule, name: &str) -> Option<String> {
    if name.is_empty() || name.ends_with(SUFFIX) {
        return None;
    }
    let suffix = format!("{SUFFIX}{}", rule.player);
    if name.ends_with(&suffix) {
        return None;
    }
    if rule.all || rule.names.iter().any(|n| matches(n, name)) {
        Some(format!("{name}{suffix}"))
    } else {
        None
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::sync::RwLock;

    use crate::detour_ptr;
    use crate::hooks::loader::{ansi_to_string, wide_to_string};
    use crate::hooks::mh::{self, Orig};
    use crate::hooks::state;

    thread_local! {
        /// Non-zero while we are inside an original call we already renamed for, so the
        /// inner `*ExW` that kernelbase forwards to does not rename a second time.
        static DEPTH: Cell<u32> = const { Cell::new(0) };
    }

    struct Guard;

    impl Guard {
        fn enter() -> Guard {
            DEPTH.with(|d| d.set(d.get() + 1));
            Guard
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            DEPTH.with(|d| d.set(d.get() - 1));
        }
    }

    fn current_rule() -> Option<RenameRule> {
        if DEPTH.with(|d| d.get()) > 0 {
            return None;
        }
        state::with(|s| RenameRule {
            player: s.player,
            names: s.flags.rename_handles.clone(),
            all: s.flags.rename_all_handles,
        })
    }

    /// Cache of the last rule to avoid cloning the name list on every kernel call.
    static RULE: RwLock<Option<RenameRule>> = RwLock::new(None);

    fn rule_for(name: &str) -> Option<String> {
        if DEPTH.with(|d| d.get()) > 0 {
            return None;
        }
        {
            let cached = RULE.read().ok()?;
            if let Some(r) = cached.as_ref() {
                return renamed(r, name);
            }
        }
        let fresh = current_rule()?;
        let result = renamed(&fresh, name);
        if let Ok(mut w) = RULE.write() {
            *w = Some(fresh);
        }
        result
    }

    /// Drop the cached rule so the next call re-reads the (changed) config.
    pub fn on_config_changed() {
        if let Ok(mut w) = RULE.write() {
            *w = None;
        }
    }

    pub trait NameKind {
        type Ptr: Copy;
        type Unit;
        /// A replacement NUL-terminated buffer if the name must change.
        unsafe fn renamed_buf(p: Self::Ptr) -> Option<Vec<Self::Unit>>;
        fn as_ptr(buf: &[Self::Unit]) -> Self::Ptr;
    }

    pub struct Wide;
    pub struct Ansi;

    impl NameKind for Wide {
        type Ptr = *const u16;
        type Unit = u16;
        unsafe fn renamed_buf(p: *const u16) -> Option<Vec<u16>> {
            if p.is_null() {
                return None;
            }
            let new = rule_for(&wide_to_string(p))?;
            Some(new.encode_utf16().chain(std::iter::once(0)).collect())
        }
        fn as_ptr(buf: &[u16]) -> *const u16 {
            buf.as_ptr()
        }
    }

    impl NameKind for Ansi {
        type Ptr = *const u8;
        type Unit = u8;
        unsafe fn renamed_buf(p: *const u8) -> Option<Vec<u8>> {
            if p.is_null() {
                return None;
            }
            let new = rule_for(&ansi_to_string(p))?;
            let mut v = new.into_bytes();
            v.push(0);
            Some(v)
        }
        fn as_ptr(buf: &[u8]) -> *const u8 {
            buf.as_ptr()
        }
    }

    /// Declares the original slot, the detour and an installer for one kernel32 export whose
    /// signature is `(before..., name, after...) -> ret`.
    macro_rules! rename_hook {
        ($orig:ident, $detour:ident, $install:ident, $kind:ty, $sym:literal,
         ($($b:ident: $bt:ty),*), ($($a:ident: $at:ty),*), $ret:ty, $fail:expr) => {
            static $orig: Orig<
                unsafe extern "system" fn($($bt,)* <$kind as NameKind>::Ptr, $($at,)*) -> $ret,
            > = Orig::new();

            unsafe extern "system" fn $detour(
                $($b: $bt,)* name: <$kind as NameKind>::Ptr, $($a: $at,)*
            ) -> $ret {
                let Some(orig) = $orig.get() else { return $fail };
                if let Some(buf) = <$kind as NameKind>::renamed_buf(name) {
                    let _g = Guard::enter();
                    return orig($($b,)* <$kind as NameKind>::as_ptr(&buf), $($a,)*);
                }
                orig($($b,)* name, $($a,)*)
            }

            fn $install() -> Result<(), String> {
                unsafe { mh::hook_export("kernel32.dll", $sym, detour_ptr!($detour), &$orig) }
            }
        };
    }

    type P = *mut c_void;
    const NULL: *mut c_void = std::ptr::null_mut();

    rename_hook!(CREATE_MUTEX_W, create_mutex_w, i_create_mutex_w, Wide, "CreateMutexW",
        (attrs: P, initial: i32), (), P, NULL);
    rename_hook!(CREATE_MUTEX_A, create_mutex_a, i_create_mutex_a, Ansi, "CreateMutexA",
        (attrs: P, initial: i32), (), P, NULL);
    rename_hook!(CREATE_MUTEX_EX_W, create_mutex_ex_w, i_create_mutex_ex_w, Wide, "CreateMutexExW",
        (attrs: P), (flags: u32, access: u32), P, NULL);
    rename_hook!(CREATE_MUTEX_EX_A, create_mutex_ex_a, i_create_mutex_ex_a, Ansi, "CreateMutexExA",
        (attrs: P), (flags: u32, access: u32), P, NULL);
    rename_hook!(OPEN_MUTEX_W, open_mutex_w, i_open_mutex_w, Wide, "OpenMutexW",
        (access: u32, inherit: i32), (), P, NULL);
    rename_hook!(OPEN_MUTEX_A, open_mutex_a, i_open_mutex_a, Ansi, "OpenMutexA",
        (access: u32, inherit: i32), (), P, NULL);

    rename_hook!(CREATE_EVENT_W, create_event_w, i_create_event_w, Wide, "CreateEventW",
        (attrs: P, manual: i32, initial: i32), (), P, NULL);
    rename_hook!(CREATE_EVENT_A, create_event_a, i_create_event_a, Ansi, "CreateEventA",
        (attrs: P, manual: i32, initial: i32), (), P, NULL);
    rename_hook!(CREATE_EVENT_EX_W, create_event_ex_w, i_create_event_ex_w, Wide, "CreateEventExW",
        (attrs: P), (flags: u32, access: u32), P, NULL);
    rename_hook!(CREATE_EVENT_EX_A, create_event_ex_a, i_create_event_ex_a, Ansi, "CreateEventExA",
        (attrs: P), (flags: u32, access: u32), P, NULL);
    rename_hook!(OPEN_EVENT_W, open_event_w, i_open_event_w, Wide, "OpenEventW",
        (access: u32, inherit: i32), (), P, NULL);

    rename_hook!(CREATE_SEMAPHORE_W, create_semaphore_w, i_create_semaphore_w, Wide, "CreateSemaphoreW",
        (attrs: P, initial: i32, max: i32), (), P, NULL);
    rename_hook!(CREATE_SEMAPHORE_A, create_semaphore_a, i_create_semaphore_a, Ansi, "CreateSemaphoreA",
        (attrs: P, initial: i32, max: i32), (), P, NULL);
    rename_hook!(CREATE_SEMAPHORE_EX_W, create_semaphore_ex_w, i_create_semaphore_ex_w, Wide, "CreateSemaphoreExW",
        (attrs: P, initial: i32, max: i32), (flags: u32, access: u32), P, NULL);
    rename_hook!(OPEN_SEMAPHORE_W, open_semaphore_w, i_open_semaphore_w, Wide, "OpenSemaphoreW",
        (access: u32, inherit: i32), (), P, NULL);

    rename_hook!(CREATE_FILE_MAPPING_W, create_file_mapping_w, i_create_file_mapping_w, Wide, "CreateFileMappingW",
        (file: P, attrs: P, protect: u32, size_high: u32, size_low: u32), (), P, NULL);
    rename_hook!(CREATE_FILE_MAPPING_A, create_file_mapping_a, i_create_file_mapping_a, Ansi, "CreateFileMappingA",
        (file: P, attrs: P, protect: u32, size_high: u32, size_low: u32), (), P, NULL);
    rename_hook!(OPEN_FILE_MAPPING_W, open_file_mapping_w, i_open_file_mapping_w, Wide, "OpenFileMappingW",
        (access: u32, inherit: i32), (), P, NULL);

    rename_hook!(CREATE_NAMED_PIPE_W, create_named_pipe_w, i_create_named_pipe_w, Wide, "CreateNamedPipeW",
        (), (open_mode: u32, pipe_mode: u32, max_instances: u32, out_size: u32, in_size: u32, timeout: u32, attrs: P),
        P, (-1isize) as *mut c_void);

    /// Installs one handle hook; returns its name on error.
    type Installer = fn() -> Result<(), String>;

    pub fn install() -> Result<(), Vec<String>> {
        let installers: [(&str, Installer); 19] = [
            ("CreateMutexW", i_create_mutex_w),
            ("CreateMutexA", i_create_mutex_a),
            ("CreateMutexExW", i_create_mutex_ex_w),
            ("CreateMutexExA", i_create_mutex_ex_a),
            ("OpenMutexW", i_open_mutex_w),
            ("OpenMutexA", i_open_mutex_a),
            ("CreateEventW", i_create_event_w),
            ("CreateEventA", i_create_event_a),
            ("CreateEventExW", i_create_event_ex_w),
            ("CreateEventExA", i_create_event_ex_a),
            ("OpenEventW", i_open_event_w),
            ("CreateSemaphoreW", i_create_semaphore_w),
            ("CreateSemaphoreA", i_create_semaphore_a),
            ("CreateSemaphoreExW", i_create_semaphore_ex_w),
            ("OpenSemaphoreW", i_open_semaphore_w),
            ("CreateFileMappingW", i_create_file_mapping_w),
            ("CreateFileMappingA", i_create_file_mapping_a),
            ("OpenFileMappingW", i_open_file_mapping_w),
            ("CreateNamedPipeW", i_create_named_pipe_w),
        ];
        let mut errors = Vec::new();
        let mut ok = 0;
        for (name, install) in installers {
            match install() {
                Ok(()) => ok += 1,
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        if ok == 0 {
            Err(errors)
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
pub use imp::{install, on_config_changed};

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(names: &[&str], all: bool) -> RenameRule {
        RenameRule {
            player: 2,
            names: names.iter().map(|s| s.to_string()).collect(),
            all,
        }
    }

    #[test]
    fn listed_names_get_the_player_suffix() {
        let r = rule(&["Global\\MyGameMutex"], false);
        assert_eq!(
            renamed(&r, "Global\\MyGameMutex").as_deref(),
            Some("Global\\MyGameMutex_LSA2")
        );
        assert_eq!(renamed(&r, "Global\\Other"), None);
    }

    #[test]
    fn matching_ignores_case_and_namespace_prefix() {
        let r = rule(&["mygamemutex"], false);
        assert!(renamed(&r, "Global\\MyGameMutex").is_some());
        assert!(renamed(&r, "Local\\MYGAMEMUTEX").is_some());
        assert!(renamed(&r, "Session\\1\\MyGameMutex").is_some());
        assert_eq!(
            renamed(&r, "\\\\.\\pipe\\MyGameMutex").as_deref(),
            Some("\\\\.\\pipe\\MyGameMutex_LSA2")
        );
    }

    #[test]
    fn aggressive_mode_renames_everything_but_never_twice() {
        let r = rule(&[], true);
        assert_eq!(renamed(&r, "Anything").as_deref(), Some("Anything_LSA2"));
        assert_eq!(renamed(&r, "Anything_LSA2"), None);
        assert_eq!(renamed(&r, ""), None);
    }

    #[test]
    fn empty_rule_renames_nothing() {
        let r = rule(&[], false);
        assert_eq!(renamed(&r, "Global\\MyGameMutex"), None);
    }
}

//! Import Address Table (IAT) hooking.
//!
//! An export hook only catches a call that actually reaches the exported function. When
//! another component patches a module's IAT to point an import somewhere else entirely — as
//! Steam's overlay does for the XInput functions, routing them into its own gamepad emulation
//! — the export is never called and the export hook is dead. Rewriting the IAT slot itself
//! catches the call wherever the import currently points, and hands us whatever was there (the
//! emulation) to call through to.
//!
//! The PE import table is parsed by hand from raw offsets rather than the `windows` crate's
//! union-heavy image structs. The loaded module is this process's own image of the same
//! bitness as this DLL, so the layout is fixed and the `cfg` below picks the right offsets.

use std::ffi::c_void;

/// One imported symbol, as the import table names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Import<'a> {
    Name(&'a str),
    Ordinal(u16),
}

// Offset of DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT] within the optional header, and the
// "import by ordinal" high bit, both bitness-dependent.
#[cfg(target_pointer_width = "64")]
const DATADIR_IMPORT_OFFSET: usize = 120;
#[cfg(target_pointer_width = "64")]
const ORDINAL_FLAG: usize = 1 << 63;
#[cfg(target_pointer_width = "32")]
const DATADIR_IMPORT_OFFSET: usize = 104;
#[cfg(target_pointer_width = "32")]
const ORDINAL_FLAG: usize = 1 << 31;

const PTR: usize = std::mem::size_of::<usize>();

unsafe fn read_u32(p: *const u8) -> u32 {
    (p as *const u32).read_unaligned()
}

unsafe fn cstr(p: *const u8) -> String {
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf8_lossy(std::slice::from_raw_parts(p, len)).into_owned()
}

/// Lower-case file stem of a module name (`XINPUT1_4.dll` -> `xinput1_4`).
fn stem(name: &str) -> String {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if base[i + 1..].eq_ignore_ascii_case("dll") => base[..i].to_ascii_lowercase(),
        _ => base.to_ascii_lowercase(),
    }
}

/// Walk `module_base`'s import table and call `visit` for every imported symbol whose source
/// DLL's stem satisfies `dll_pred`, with a pointer to that symbol's IAT slot.
///
/// # Safety
/// `module_base` must be the load address of a well-formed PE image mapped in this process.
pub unsafe fn visit_iat(
    module_base: *const u8,
    dll_pred: impl Fn(&str) -> bool,
    mut visit: impl FnMut(&str, Import<'_>, *mut *mut c_void),
) {
    if module_base.is_null() {
        return;
    }
    // DOS header: "MZ", e_lfanew at 0x3C.
    if read_u32(module_base) & 0xFFFF != 0x5A4D {
        return;
    }
    let e_lfanew = read_u32(module_base.add(0x3C)) as usize;
    let nt = module_base.add(e_lfanew);
    if read_u32(nt) != 0x0000_4550 {
        // "PE\0\0"
        return;
    }
    // Optional header begins after Signature (4) + IMAGE_FILE_HEADER (20).
    let opt = nt.add(24);
    let import_rva = read_u32(opt.add(DATADIR_IMPORT_OFFSET)) as usize;
    if import_rva == 0 {
        return;
    }

    // Array of 20-byte IMAGE_IMPORT_DESCRIPTOR, terminated by an all-zero entry.
    let mut desc = module_base.add(import_rva);
    loop {
        let original_first_thunk = read_u32(desc) as usize;
        let name_rva = read_u32(desc.add(12)) as usize;
        let first_thunk = read_u32(desc.add(16)) as usize;
        if name_rva == 0 && first_thunk == 0 {
            break;
        }
        let dll = cstr(module_base.add(name_rva));
        let dll_stem = stem(&dll);
        if first_thunk != 0 && dll_pred(&dll_stem) {
            // The INT (names) parallels the IAT (pointers); bound imports have no INT, so fall
            // back to reading names from the IAT itself.
            let names_rva = if original_first_thunk != 0 {
                original_first_thunk
            } else {
                first_thunk
            };
            let mut i = 0usize;
            loop {
                let int_entry = (module_base.add(names_rva + i * PTR) as *const usize)
                    .read_unaligned();
                if int_entry == 0 {
                    break;
                }
                let slot = module_base.add(first_thunk + i * PTR) as *mut *mut c_void;
                if int_entry & ORDINAL_FLAG != 0 {
                    visit(&dll_stem, Import::Ordinal((int_entry & 0xFFFF) as u16), slot);
                } else {
                    // int_entry is an RVA to IMAGE_IMPORT_BY_NAME { Hint: u16, Name: cstr }.
                    let name = cstr(module_base.add(int_entry + 2));
                    visit(&dll_stem, Import::Name(&name), slot);
                }
                i += 1;
            }
        }
        desc = desc.add(20);
    }
}

/// Overwrite an IAT slot, returning its previous value. `None` if the memory could not be made
/// writable.
///
/// # Safety
/// `slot` must point to a writable-after-`VirtualProtect` pointer inside an IAT.
pub unsafe fn write_slot(slot: *mut *mut c_void, value: *mut c_void) -> Option<*mut c_void> {
    use windows::Win32::System::Memory::{
        VirtualProtect, PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
    };
    let mut old = PAGE_PROTECTION_FLAGS(0);
    VirtualProtect(slot as *const c_void, PTR, PAGE_READWRITE, &mut old).ok()?;
    let previous = *slot;
    *slot = value;
    let mut back = PAGE_PROTECTION_FLAGS(0);
    let _ = VirtualProtect(slot as *const c_void, PTR, old, &mut back);
    Some(previous)
}

/// Load addresses of every module currently mapped in this process.
pub fn modules() -> Vec<*const u8> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W,
        TH32CS_SNAPMODULE,
    };
    use windows::Win32::System::Threading::GetCurrentProcessId;

    let mut out = Vec::new();
    unsafe {
        let pid = GetCurrentProcessId();
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) else {
            return out;
        };
        let mut entry = MODULEENTRY32W {
            dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
            ..Default::default()
        };
        if Module32FirstW(snap, &mut entry).is_ok() {
            loop {
                out.push(entry.modBaseAddr as *const u8);
                if Module32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_lowercases_and_drops_dll() {
        assert_eq!(stem("XINPUT1_4.dll"), "xinput1_4");
        assert_eq!(stem(r"C:\x\xinput9_1_0.DLL"), "xinput9_1_0");
        assert_eq!(stem("GameOverlayRenderer64.dll"), "gameoverlayrenderer64");
    }
}

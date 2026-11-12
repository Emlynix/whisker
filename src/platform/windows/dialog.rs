//! The system's "Open" and "Save as" dialogs (`comdlg32`).

use super::win::wide;
use crate::shell::FileAsk;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::Dialogs::*;

/// Room for the chosen path, in UTF-16 units (long paths included).
const PATH_CHARS: usize = 1024;

/// Asks for a file with the extension `ext`; `name` is the proposed file
/// name. Blocks until the dialog closes; `None` if it was cancelled.
pub fn ask(owner: HWND, ask: FileAsk, title: &str, name: &str, ext: &str) -> Option<PathBuf> {
    let mut file = [0u16; PATH_CHARS];
    for (slot, unit) in file.iter_mut().zip(name.encode_utf16().take(PATH_CHARS - 1)) {
        *slot = unit;
    }
    // "Whisker (*.ext)\0*.ext\0\0": one filter, closed by a second NUL.
    let filter = wide(&format!("{} (*.{ext})\0*.{ext}\0", crate::APP_NAME));
    let (title, ext) = (wide(title), wide(ext));

    let mut o: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    o.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    o.hwndOwner = owner;
    o.lpstrFilter = filter.as_ptr();
    o.lpstrFile = file.as_mut_ptr();
    o.nMaxFile = PATH_CHARS as u32;
    o.lpstrTitle = title.as_ptr();
    o.lpstrDefExt = ext.as_ptr();
    // The process keeps its working directory whatever folder is browsed.
    o.Flags = OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_HIDEREADONLY;

    // SAFETY: every pointer in `o` outlives the call; `file` has `nMaxFile` units.
    let chosen = unsafe {
        match ask {
            FileAsk::Open => {
                o.Flags |= OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
                GetOpenFileNameW(&mut o)
            }
            FileAsk::Save => {
                o.Flags |= OFN_OVERWRITEPROMPT;
                GetSaveFileNameW(&mut o)
            }
        }
    };
    if chosen == 0 {
        return None;
    }
    let len = file.iter().position(|&u| u == 0).unwrap_or(PATH_CHARS);
    Some(PathBuf::from(String::from_utf16_lossy(&file[..len])))
}

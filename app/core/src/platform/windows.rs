//! Win32 calls: a named mutex for single instance, the per-user `Run` key
//! for start at login, Steam's install path and Battle.net's uninstall
//! entries, and Battle.net process/window checks. No admin rights needed.

use std::io;
use std::path::PathBuf;
use std::ptr;

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND,
    ERROR_MORE_DATA, ERROR_SUCCESS, HANDLE,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, RegSetKeyValueW,
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    REG_EXPAND_SZ, REG_SZ, RRF_NOEXPAND, RRF_RT_ANY,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, OpenMutexW, SetEvent, WaitForSingleObject,
    EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// NUL-terminated UTF-16 for `PCWSTR` arguments.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn check(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

/// Handle to a named mutex; the name is released when the last handle closes
/// (on drop, or when the process exits).
pub(crate) struct NamedMutex(HANDLE);

// The handle is a plain kernel object reference, usable from any thread.
unsafe impl Send for NamedMutex {}

impl NamedMutex {
    /// `Ok(None)` if another handle to `name` already exists.
    pub(crate) fn acquire(name: &str) -> io::Result<Option<Self>> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and outlives the call; null
        // attributes = default security.
        let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        // SAFETY: no preconditions; read before any other call can overwrite it.
        let err = unsafe { GetLastError() };
        if handle.is_null() {
            // Access denied: the mutex exists but its owner restricts it, so
            // someone else holds the name.
            return match err {
                ERROR_ACCESS_DENIED => Ok(None),
                _ => Err(io::Error::from_raw_os_error(err as i32)),
            };
        }
        let mutex = Self(handle);
        Ok((err != ERROR_ALREADY_EXISTS).then_some(mutex))
    }

    /// Whether `name` exists, without creating it (so a concurrent
    /// [`acquire`](Self::acquire) never sees this probe as the owner).
    pub(crate) fn exists(name: &str) -> io::Result<bool> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and outlives the call.
        let handle = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
        if !handle.is_null() {
            // SAFETY: we own the handle just opened and close it once.
            unsafe { CloseHandle(handle) };
            return Ok(true);
        }
        // SAFETY: no preconditions; read right after the failing call.
        match unsafe { GetLastError() } {
            ERROR_FILE_NOT_FOUND => Ok(false),
            ERROR_ACCESS_DENIED => Ok(true),
            err => Err(io::Error::from_raw_os_error(err as i32)),
        }
    }
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// A named auto-reset event: one process waits on it, others signal it.
pub(crate) struct NamedEvent(HANDLE);

// A plain kernel object reference, usable from any thread.
unsafe impl Send for NamedEvent {}

impl NamedEvent {
    /// Creates `name` (or opens it if it exists), unsignaled.
    pub(crate) fn create(name: &str) -> io::Result<Self> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and outlives the call; null
        // attributes = default security; auto-reset, initially unsignaled.
        let handle = unsafe { CreateEventW(ptr::null(), 0, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    /// Whether the event was signaled since the last call (that resets it).
    pub(crate) fn take(&self) -> bool {
        // SAFETY: we own the handle; a zero timeout never blocks.
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }

    /// Signals `name`; `Ok(false)` if no process has it open.
    pub(crate) fn signal(name: &str) -> io::Result<bool> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and outlives the call.
        let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if handle.is_null() {
            return match io::Error::last_os_error() {
                e if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => Ok(false),
                e => Err(e),
            };
        }
        // SAFETY: the handle was just opened with EVENT_MODIFY_STATE.
        let ok = unsafe { SetEvent(handle) } != 0;
        let result = if ok {
            Ok(true)
        } else {
            Err(io::Error::last_os_error())
        };
        // SAFETY: we own the handle just opened and close it once.
        unsafe { CloseHandle(handle) };
        result
    }
}

impl Drop for NamedEvent {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// `HKCU\...\Run\<name>`, or `Ok(None)` if the value does not exist.
/// A `REG_EXPAND_SZ` value is returned raw (unexpanded); any other non-string
/// type is an `InvalidData` error naming the type.
pub(crate) fn run_value(name: &str) -> Option<io::Result<Option<String>>> {
    Some(
        reg_string(HKEY_CURRENT_USER, RUN_KEY, name).map_err(|e| match e.kind() {
            io::ErrorKind::InvalidData => io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{e}; `autostart enable` replaces it"),
            ),
            _ => e,
        }),
    )
}

/// Steam install folders from the registry, most specific first.
pub(crate) fn steam_paths() -> Vec<PathBuf> {
    let hklm_key = r"SOFTWARE\WOW6432Node\Valve\Steam";
    [
        (HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath"),
        (HKEY_LOCAL_MACHINE, hklm_key, "InstallPath"),
    ]
    .into_iter()
    .filter_map(|(root, key, name)| reg_string(root, key, name).ok().flatten())
    .filter(|v| !v.is_empty())
    .map(PathBuf::from)
    .collect()
}

const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
/// Uninstall subkeys read per view; a sane bound for untrusted data.
const MAX_UNINSTALL_KEYS: u32 = 10_000;

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Key {
    /// `None` if the key does not exist or cannot be read.
    fn open(root: HKEY, path: &str, view: u32) -> Option<Key> {
        let path = wide(path);
        let mut key: HKEY = ptr::null_mut();
        // SAFETY: `path` is NUL-terminated; `key` is a valid out pointer.
        let code = unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ | view, &mut key) };
        (code == ERROR_SUCCESS).then_some(Key(key))
    }

    /// The names of its direct subkeys (at most `max`).
    fn subkeys(&self, max: u32) -> Vec<String> {
        let mut names = Vec::new();
        // Key names are at most 255 characters.
        let mut buf = [0u16; 256];
        for index in 0..max {
            let mut len = buf.len() as u32;
            // SAFETY: `buf` holds `len` characters; the optional out
            // pointers are null.
            let code = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    buf.as_mut_ptr(),
                    &mut len,
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if code != ERROR_SUCCESS {
                // ERROR_NO_MORE_ITEMS, or a key that vanished meanwhile.
                break;
            }
            names.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        names
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: we own the open key and close it exactly once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// The uninstall entries of Battle.net games, from the 32- and 64-bit
/// machine views and the user's own.
pub(crate) fn blizzard_uninstalls() -> Vec<crate::battlenet::Uninstall> {
    use crate::battlenet::{is_blizzard_uninstaller, uninstall_uid, Uninstall};
    let views = [
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
        (HKEY_CURRENT_USER, 0),
    ];
    let mut found: Vec<Uninstall> = Vec::new();
    for (root, view) in views {
        let Some(key) = Key::open(root, UNINSTALL_KEY, view) else {
            continue;
        };
        for name in key.subkeys(MAX_UNINSTALL_KEYS) {
            let value = |v| reg_string(key.0, &name, v).ok().flatten();
            let Some(command) = value("UninstallString") else {
                continue;
            };
            let location = value("InstallLocation").filter(|l| !l.trim().is_empty());
            if let (true, Some(location)) = (is_blizzard_uninstaller(&command), location) {
                let entry = Uninstall {
                    location: PathBuf::from(location.trim().trim_matches('"')),
                    uid: uninstall_uid(&command),
                };
                if !found.contains(&entry) {
                    found.push(entry);
                }
            }
        }
    }
    found
}

/// `%ProgramFiles(x86)%` and `%ProgramFiles%`.
pub(crate) fn program_files() -> Vec<PathBuf> {
    ["ProgramFiles(x86)", "ProgramFiles"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Where Battle.net may be installed: its uninstall entry's folder, then
/// the default folders.
pub(crate) fn battlenet_dirs() -> Vec<PathBuf> {
    let registered = [KEY_WOW64_32KEY, KEY_WOW64_64KEY]
        .into_iter()
        .filter_map(|view| {
            let key = Key::open(HKEY_LOCAL_MACHINE, UNINSTALL_KEY, view)?;
            let location = reg_string(key.0, "Battle.net", "InstallLocation").ok()??;
            let location = location.trim().trim_matches('"');
            (!location.is_empty()).then(|| PathBuf::from(location))
        });
    let defaults = program_files().into_iter().map(|p| p.join("Battle.net"));
    registered.chain(defaults).collect()
}

/// A string value, or `Ok(None)` if the key or value does not exist.
fn reg_string(root: HKEY, key: &str, name: &str) -> io::Result<Option<String>> {
    let (key, name) = (wide(key), wide(name));
    let mut buf = vec![0u16; 1024];
    loop {
        let mut len = (buf.len() * 2) as u32;
        let mut kind = 0u32;
        // SAFETY: strings are NUL-terminated; `buf` holds `len` bytes; `kind`
        // is a valid out pointer.
        let code = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_ANY | RRF_NOEXPAND,
                &mut kind,
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        match code {
            ERROR_MORE_DATA => buf.resize(len as usize / 2 + 1, 0),
            ERROR_FILE_NOT_FOUND => return Ok(None),
            ERROR_SUCCESS if kind != REG_SZ && kind != REG_EXPAND_SZ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the value is not a string (registry type {kind})"),
                ));
            }
            ERROR_SUCCESS => {
                // String types are NUL-terminated by RegGetValueW; `len`
                // includes the NUL.
                let chars = &buf[..(len as usize / 2).saturating_sub(1)];
                return Ok(Some(String::from_utf16_lossy(chars)));
            }
            code => return Err(io::Error::from_raw_os_error(code as i32)),
        }
    }
}

pub(crate) fn set_run_value(name: &str, value: &str) -> Option<io::Result<()>> {
    let (key, name, data) = (wide(RUN_KEY), wide(name), wide(value));
    // SAFETY: strings are NUL-terminated; size is in bytes, NUL included.
    let code = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    Some(check(code))
}

/// Deleting a value that does not exist is not an error.
pub(crate) fn delete_run_value(name: &str) -> Option<io::Result<()>> {
    let (key, name) = (wide(RUN_KEY), wide(name));
    // SAFETY: strings are NUL-terminated.
    let code = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
    Some(if code == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        check(code)
    })
}

/// Ids of the running processes whose executable is named `exe_name`
/// (case-insensitive, e.g. `Battle.net.exe`).
pub(crate) fn process_ids(exe_name: &str) -> Vec<u32> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: no pointer arguments; the handle is checked and closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
        tracing::debug!("process snapshot failed: {}", io::Error::last_os_error());
        return Vec::new();
    }
    let mut ids = Vec::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: `snapshot` is a valid process snapshot and `entry` is a
    // `PROCESSENTRY32W` with `dwSize` set, as both calls require.
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
        if name.eq_ignore_ascii_case(exe_name) {
            ids.push(entry.th32ProcessID);
        }
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    // SAFETY: `snapshot` is a handle we own and close once.
    unsafe { CloseHandle(snapshot) };
    ids
}

/// Whether any process in `pids` owns a visible top-level window.
pub(crate) fn has_visible_window(pids: &[u32]) -> bool {
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
    };

    struct Search<'a> {
        pids: &'a [u32],
        found: bool,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `&mut Search` passed to `EnumWindows`
        // below, which outlives the enumeration.
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut pid = 0u32;
        // SAFETY: `hwnd` comes from `EnumWindows`; `pid` is a valid out pointer.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        // SAFETY: `hwnd` comes from `EnumWindows`.
        if search.pids.contains(&pid) && unsafe { IsWindowVisible(hwnd) } != 0 {
            search.found = true;
            return 0; // stop enumerating
        }
        1
    }

    if pids.is_empty() {
        return false;
    }
    let mut search = Search { pids, found: false };
    // SAFETY: `visit` matches `WNDENUMPROC`; `search` outlives the call.
    // The return value is 0 both on error and when `visit` stops early, so
    // `found` is the answer either way.
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    search.found
}

/// The largest version-info block read (real ones are a few KB).
const MAX_VERSION_INFO: u32 = 1 << 20;

/// The exe's `FileDescription` and `ProductName`, in the language its
/// `\VarFileInfo\Translation` lists first, else US English.
pub(crate) fn version_info(path: &std::path::Path) -> Option<crate::program::VersionInfo> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW};

    let path: Vec<u16> = path.as_os_str().encode_wide().collect();
    if path.contains(&0) {
        return None;
    }
    let path: Vec<u16> = path.into_iter().chain([0]).collect();
    let mut handle = 0;
    // SAFETY: `path` is NUL-terminated and outlives the call.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut handle) };
    if size == 0 || size > MAX_VERSION_INFO {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    // SAFETY: `block` is `size` bytes, as the call is told. The file is
    // loaded as data only; none of its code runs.
    let ok = unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, block.as_mut_ptr().cast()) };
    if ok == 0 {
        return None;
    }
    let mut languages: Vec<String> = query(&block, r"\VarFileInfo\Translation", 1)
        .unwrap_or_default()
        .as_chunks::<4>()
        .0
        .iter()
        .take(8)
        .map(|p| {
            let lang = u16::from_le_bytes([p[0], p[1]]);
            let codepage = u16::from_le_bytes([p[2], p[3]]);
            format!("{lang:04X}{codepage:04X}")
        })
        .collect();
    languages.extend(["040904B0".into(), "040904E4".into()]);
    let string = |name: &str| {
        languages.iter().find_map(|l| {
            let bytes = query(&block, &format!(r"\StringFileInfo\{l}\{name}"), 2)?;
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .take_while(|&u| u != 0)
                .take(1024)
                .collect();
            let s = String::from_utf16_lossy(&units);
            (!s.trim().is_empty()).then_some(s)
        })
    };
    Some(crate::program::VersionInfo {
        file_description: string("FileDescription"),
        product_name: string("ProductName"),
    })
}

/// The bytes `VerQueryValueW` finds at `sub` in `block`, clipped to it.
/// Its length comes in `unit`s: UTF-16 units (2) for strings, bytes (1)
/// for the translation table.
fn query<'b>(block: &'b [u8], sub: &str, unit: usize) -> Option<&'b [u8]> {
    use windows_sys::Win32::Storage::FileSystem::VerQueryValueW;

    let sub = wide(sub);
    let mut ptr = ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: `block` came from GetFileVersionInfoW; `sub` is NUL-terminated;
    // the out pointers are valid. The result points into `block`.
    let ok = unsafe { VerQueryValueW(block.as_ptr().cast(), sub.as_ptr(), &mut ptr, &mut len) };
    if ok == 0 || ptr.is_null() {
        return None;
    }
    let start = (ptr as usize).checked_sub(block.as_ptr() as usize)?;
    if start >= block.len() {
        return None;
    }
    let end = start
        .saturating_add((len as usize).saturating_mul(unit))
        .min(block.len());
    Some(&block[start..end])
}

//! Win32 calls: a named mutex for single instance, the per-user `Run` key
//! for start at login and Steam's install path. No admin rights needed.

use std::io;
use std::path::PathBuf;
use std::ptr;

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND,
    ERROR_MORE_DATA, ERROR_SUCCESS, HANDLE,
};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
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

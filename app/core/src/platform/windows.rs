//! Win32 calls: a named mutex for single instance and the per-user `Run` key
//! for start at login. No admin rights needed for either.

use std::io;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA,
    ERROR_SUCCESS, HANDLE,
};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Threading::CreateMutexW;

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
        // Read before any other call can overwrite it.
        let err = unsafe { GetLastError() };
        if handle.is_null() {
            return Err(io::Error::from_raw_os_error(err as i32));
        }
        let mutex = Self(handle);
        Ok((err != ERROR_ALREADY_EXISTS).then_some(mutex))
    }
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// `HKCU\...\Run\<name>`, or `Ok(None)` if the value does not exist.
pub(crate) fn run_value(name: &str) -> Option<io::Result<Option<String>>> {
    let (key, name) = (wide(RUN_KEY), wide(name));
    let mut buf = vec![0u16; 1024];
    loop {
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: strings are NUL-terminated; `buf` holds `len` bytes.
        let code = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        match code {
            ERROR_MORE_DATA => buf.resize(len as usize / 2 + 1, 0),
            ERROR_FILE_NOT_FOUND => return Some(Ok(None)),
            ERROR_SUCCESS => {
                // `len` includes the terminating NUL.
                let chars = &buf[..(len as usize / 2).saturating_sub(1)];
                return Some(Ok(Some(String::from_utf16_lossy(chars))));
            }
            code => return Some(Err(io::Error::from_raw_os_error(code as i32))),
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

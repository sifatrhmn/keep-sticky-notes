//! Stores the master token in Windows Credential Manager (protected by DPAPI).

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

const TARGET: &str = "KeepStickyNotes/GoogleMasterToken";

pub fn save(email: &str, token: &str) -> windows::core::Result<()> {
    let target = HSTRING::from(TARGET);
    let user = HSTRING::from(email);
    let mut blob = token.as_bytes().to_vec();
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_ptr() as *mut _),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: PWSTR(user.as_ptr() as *mut _),
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0) }
}

/// Returns the signed-in email address and master token, if any.
pub fn load() -> Option<(String, String)> {
    let target = HSTRING::from(TARGET);
    let mut out: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(&target, CRED_TYPE_GENERIC, None, &mut out).ok()?;
        let c = &*out;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let token = String::from_utf8_lossy(bytes).into_owned();
        let email = c.UserName.to_string().unwrap_or_default();
        CredFree(out as *const _);
        Some((email, token))
    }
}

pub fn delete() {
    let target = HSTRING::from(TARGET);
    unsafe {
        let _ = CredDeleteW(&target, CRED_TYPE_GENERIC, None);
    }
}

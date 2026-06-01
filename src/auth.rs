//! PAM authentication for lock screen
//! Uses raw FFI to call Linux PAM functions — no extra crate needed.

use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

// PAM constants
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_SUCCESS: c_int = 0;

#[repr(C)]
struct PamConv {
    conv: Option<
        unsafe extern "C" fn(
            c_int,
            *mut *mut PamMessage,
            *mut *mut PamResponse,
            *mut c_void,
        ) -> c_int,
    >,
    appdata_ptr: *mut c_void,
}

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *mut c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

thread_local! {
    static PAM_PASSWORD: RefCell<Option<String>> = RefCell::new(None);
}

unsafe extern "C" fn pam_conv_callback(
    num_msg: c_int,
    msg: *mut *mut PamMessage,
    resp: *mut *mut PamResponse,
    _appdata_ptr: *mut c_void,
) -> c_int {
    if num_msg <= 0 {
        return 2; // PAM_BUF_ERR
    }

    let responses =
        libc::calloc(num_msg as usize, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
    if responses.is_null() {
        return 2;
    }

    for i in 0..num_msg as isize {
        let msg_entry = *msg.offset(i);
        let style = (*msg_entry).msg_style;

        match style {
            PAM_PROMPT_ECHO_OFF | 2 /* PAM_PROMPT_ECHO_ON */ => {
                let password = PAM_PASSWORD.with(|p| p.borrow().clone()).unwrap_or_default();
                if let Ok(c_pass) = CString::new(password) {
                    (*responses.offset(i)).resp = libc::strdup(c_pass.as_ptr()) as *mut c_char;
                }
                (*responses.offset(i)).resp_retcode = 0;
            }
            _ => {
                (*responses.offset(i)).resp = ptr::null_mut();
                (*responses.offset(i)).resp_retcode = 0;
            }
        }
    }

    *resp = responses;
    0 // PAM_SUCCESS
}

/// Verify a password against the system PAM for the given username.
/// Returns `true` if authentication succeeds.
pub fn verify_password(username: &str, password: &str) -> bool {
    let c_service = match CString::new("login") {
        Ok(s) => s,
        Err(_) => return false,
    };
    let c_user = match CString::new(username) {
        Ok(s) => s,
        Err(_) => return false,
    };

    // Store password in thread-local for the conversation callback
    PAM_PASSWORD.with(|p| *p.borrow_mut() = Some(password.to_string()));

    let conv = PamConv {
        conv: Some(pam_conv_callback),
        appdata_ptr: ptr::null_mut(),
    };

    let mut pamh: *mut c_void = ptr::null_mut();

    unsafe {
        let start_result = pam_start(c_service.as_ptr(), c_user.as_ptr(), &conv, &mut pamh);
        if start_result != PAM_SUCCESS {
            PAM_PASSWORD.with(|p| *p.borrow_mut() = None);
            return false;
        }

        let auth_result = pam_authenticate(pamh, 0);
        pam_end(pamh, auth_result);

        // Clear password from thread-local
        PAM_PASSWORD.with(|p| *p.borrow_mut() = None);

        auth_result == PAM_SUCCESS
    }
}

extern "C" {
    fn pam_start(
        service: *const c_char,
        user: *const c_char,
        conv: *const PamConv,
        pamh: *mut *mut c_void,
    ) -> c_int;

    fn pam_authenticate(pamh: *mut c_void, flags: c_int) -> c_int;

    fn pam_end(pamh: *mut c_void, pam_status: c_int) -> c_int;
}

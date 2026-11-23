fn encoded_target(url: &str) -> Option<Vec<u16>> {
    if !url.starts_with("https://") || url.chars().any(char::is_control) {
        return None;
    }
    Some(url.encode_utf16().chain(Some(0)).collect())
}

fn is_shell_success(result: isize) -> bool {
    result > 32
}

#[cfg(windows)]
struct ComApartment;

#[cfg(windows)]
impl ComApartment {
    fn initialize() -> Option<Self> {
        use windows_sys::Win32::System::Com::{
            CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        };
        // SAFETY: this runs on a dedicated new thread and passes the required null reserved argument.
        let result = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        };
        // [Review Fix #3] Construct the Drop guard only after success; then_some eagerly drops it on failure.
        if result >= 0 {
            Some(Self)
        } else {
            None
        }
    }
}

#[cfg(windows)]
impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: this guard exists only after successful initialization on the same thread.
        unsafe { windows_sys::Win32::System::Com::CoUninitialize() };
    }
}

#[cfg(windows)]
pub(super) fn open(url: &str) -> bool {
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let Some(target) = encoded_target(url) else {
        return false;
    };
    let Some(_apartment) = ComApartment::initialize() else {
        return false;
    };
    let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    // [Review Fix #2] Pass the HTTPS URL as a document to the default handler, never as a shell command.
    // SAFETY: both strings are NUL-terminated and alive for the call; optional pointers are null.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    is_shell_success(result as isize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_preserves_the_entire_url_and_adds_exactly_one_terminator() {
        let url = "https://accounts.doubao.com/authorize?state=a&name=测试😀&scope=x%20y";
        let encoded = encoded_target(url).unwrap();
        assert_eq!(encoded.last(), Some(&0));
        assert_eq!(encoded.iter().filter(|unit| **unit == 0).count(), 1);
        assert_eq!(
            String::from_utf16(&encoded[..encoded.len() - 1]).unwrap(),
            url
        );
    }

    #[test]
    fn non_https_targets_and_embedded_controls_are_rejected() {
        for url in [
            "",
            "file:///C:/program.exe",
            "javascript:alert(1)",
            "https://accounts.doubao.com/\0hidden",
            "https://accounts.doubao.com/\n",
        ] {
            assert!(encoded_target(url).is_none());
        }
    }

    #[test]
    fn shell_result_codes_follow_the_windows_success_boundary() {
        for result in [-1, 0, 2, 31, 32] {
            assert!(!is_shell_success(result));
        }
        for result in [33, 42, isize::MAX] {
            assert!(is_shell_success(result));
        }
    }
}

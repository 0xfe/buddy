//! Best-effort browser launching for login flows.

/// Best-effort browser opener used by `/login` and `buddy login`.
pub fn try_open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        // macOS standard browser launcher.
        return std::process::Command::new("open")
            .arg(url)
            .status()
            .is_ok_and(|status| status.success());
    }
    #[cfg(target_os = "windows")]
    {
        // Windows shell launcher.
        return std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
            .is_ok_and(|status| status.success());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Linux/BSD desktop launcher.
        return std::process::Command::new("xdg-open")
            .arg(url)
            .status()
            .is_ok_and(|status| status.success());
    }
    #[allow(unreachable_code)]
    false
}

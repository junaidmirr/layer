use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::AsRawFd;

static ALWAYS_ALLOW: AtomicBool = AtomicBool::new(false);

pub struct PermissionManager;

impl PermissionManager {
    /// Reset session permission state (used in tests and session resets)
    pub fn reset_session() {
        ALWAYS_ALLOW.store(false, Ordering::SeqCst);
    }

    /// Check if session is set to always allow
    pub fn is_always_allowed() -> bool {
        ALWAYS_ALLOW.load(Ordering::SeqCst)
    }

    /// Force session always allow setting
    pub fn set_always_allowed(val: bool) {
        ALWAYS_ALLOW.store(val, Ordering::SeqCst);
    }

    /// Check whether a tool call is permitted.
    /// - In non-interactive terminals: automatically allowed immediately with zero delay.
    /// - In interactive terminals: displays prompt on /dev/tty with a 5s countdown timer.
    ///   If no response within 5 seconds, automatically allowed.
    ///   User can enter 'n'/'no' to deny, 'y'/'yes' (or Enter) to allow, or 'a'/'always' to allow all tools for the session.
    pub fn check_permission(tool_name: &str) -> bool {
        // Fast path 1: Session is already set to always allow
        if ALWAYS_ALLOW.load(Ordering::SeqCst) {
            return true;
        }

        // Fast path 2: Explicit auto-approval environment variable
        if std::env::var("LAYER_AUTO_APPROVE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
            .unwrap_or(false)
        {
            return true;
        }

        // Fast path 3: CI and non-interactive environments
        if std::env::var("CI").is_ok()
            || std::env::var("DEBIAN_FRONTEND").as_deref() == Ok("noninteractive")
            || std::env::var("LAYER_NON_INTERACTIVE").is_ok()
            || std::env::var("NONINTERACTIVE").is_ok()
            || std::env::var("TERM").as_deref() == Ok("dumb")
        {
            return true;
        }

        #[cfg(unix)]
        {
            Self::check_permission_unix(tool_name)
        }

        #[cfg(not(unix))]
        {
            // Windows / non-unix fallback: automatically allow
            true
        }
    }

    #[cfg(unix)]
    fn check_permission_unix(tool_name: &str) -> bool {
        // Attempt to open /dev/tty for both writing prompt and reading user choice
        let mut tty_out = match OpenOptions::new().write(true).open("/dev/tty") {
            Ok(f) => f,
            Err(_) => {
                // Cannot open /dev/tty -> non-interactive session terminal: auto allow immediately
                return true;
            }
        };

        let tty_in = match File::open("/dev/tty") {
            Ok(f) => f,
            Err(_) => {
                // Cannot open /dev/tty -> non-interactive: auto allow immediately
                return true;
            }
        };

        let raw_fd = tty_in.as_raw_fd();
        if unsafe { libc::isatty(raw_fd) } == 0 {
            // Not a real TTY -> non-interactive: auto allow immediately
            return true;
        }

        // Check if our process is in the foreground process group of the controlling terminal
        let pgrp = unsafe { libc::getpgrp() };
        let tcpgrp = unsafe { libc::tcgetpgrp(raw_fd) };
        if tcpgrp != -1 && pgrp != tcpgrp {
            // Background process group -> non-interactive session: auto allow immediately
            return true;
        }

        let timeout_secs = std::env::var("LAYER_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5);

        let _ = write!(
            tty_out,
            "\n\x1b[1;36m[layer]\x1b[0m Tool call requested: \x1b[1;33m'{}'\x1b[0m\n",
            tool_name
        );
        let _ = write!(
            tty_out,
            "\x1b[1;36m[layer]\x1b[0m Allow execution? [\x1b[1;32mY\x1b[0mes] / [\x1b[1;31mn\x1b[0mo] / [\x1b[1;35ma\x1b[0mlways] (Auto-approving in {}s...): ",
            timeout_secs
        );
        let _ = tty_out.flush();

        let start = Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        let mut last_displayed = timeout_secs;

        let mut reader = BufReader::new(tty_in);

        loop {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                let _ = writeln!(
                    tty_out,
                    "\n\x1b[1;36m[layer]\x1b[0m ⏱️ Timeout expired ({}s). \x1b[1;32mAutomatically allowed\x1b[0m '{}'.\n",
                    timeout_secs, tool_name
                );
                let _ = tty_out.flush();
                return true;
            }

            let remaining = timeout_secs.saturating_sub(elapsed.as_secs());
            if remaining != last_displayed && remaining > 0 {
                last_displayed = remaining;
                let _ = write!(
                    tty_out,
                    "\r\x1b[1;36m[layer]\x1b[0m Allow execution? [\x1b[1;32mY\x1b[0mes] / [\x1b[1;31mn\x1b[0mo] / [\x1b[1;35ma\x1b[0mlways] (Auto-approving in {}s...): ",
                    remaining
                );
                let _ = tty_out.flush();
            }

            // Poll /dev/tty fd for ready input with 150ms slice
            let mut pfd = libc::pollfd {
                fd: raw_fd,
                events: libc::POLLIN,
                revents: 0,
            };

            let poll_res = unsafe { libc::poll(&mut pfd, 1, 150) };
            if poll_res > 0 && (pfd.revents & libc::POLLIN) != 0 {
                let mut line = String::new();
                if reader.read_line(&mut line).is_ok() {
                    let input = line.trim().to_lowercase();
                    if input == "n" || input == "no" || input == "deny" {
                        let _ = writeln!(
                            tty_out,
                            "\n\x1b[1;36m[layer]\x1b[0m ❌ Tool '{}' \x1b[1;31mdenied by user\x1b[0m.\n",
                            tool_name
                        );
                        let _ = tty_out.flush();
                        return false;
                    } else if input == "a" || input == "always" {
                        ALWAYS_ALLOW.store(true, Ordering::SeqCst);
                        let _ = writeln!(
                            tty_out,
                            "\n\x1b[1;36m[layer]\x1b[0m ✅ Tool '{}' \x1b[1;32mallowed\x1b[0m (Always-allow active for this session).\n",
                            tool_name
                        );
                        let _ = tty_out.flush();
                        return true;
                    } else {
                        // 'y', 'yes', or empty Enter
                        let _ = writeln!(
                            tty_out,
                            "\n\x1b[1;36m[layer]\x1b[0m ✅ Tool '{}' \x1b[1;32mallowed\x1b[0m.\n",
                            tool_name
                        );
                        let _ = tty_out.flush();
                        return true;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_interactive_session_auto_allows_immediately() {
        PermissionManager::reset_session();
        // Explicitly set non-interactive or auto-approve
        unsafe { std::env::set_var("LAYER_AUTO_APPROVE", "1") };
        let start = Instant::now();
        let allowed = PermissionManager::check_permission("write_file");
        assert!(allowed);
        assert!(start.elapsed() < Duration::from_millis(10));
        unsafe { std::env::remove_var("LAYER_AUTO_APPROVE") };
    }

    #[test]
    fn test_always_allow_flag_persists_in_session() {
        PermissionManager::reset_session();
        assert!(!PermissionManager::is_always_allowed());
        PermissionManager::set_always_allowed(true);
        assert!(PermissionManager::is_always_allowed());
        assert!(PermissionManager::check_permission("edit_file"));
        PermissionManager::reset_session();
        assert!(!PermissionManager::is_always_allowed());
    }

    #[test]
    fn test_ci_and_non_interactive_env_auto_allows() {
        PermissionManager::reset_session();
        unsafe { std::env::set_var("CI", "true") };
        assert!(PermissionManager::check_permission("write_file"));
        unsafe { std::env::remove_var("CI") };

        unsafe { std::env::set_var("LAYER_NON_INTERACTIVE", "1") };
        assert!(PermissionManager::check_permission("rename_symbol"));
        unsafe { std::env::remove_var("LAYER_NON_INTERACTIVE") };
    }
}

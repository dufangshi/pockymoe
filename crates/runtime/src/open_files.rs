//! The open-file limit of this process and of the harnesses it starts.
//!
//! A login shell or a service manager usually starts a process with a soft
//! limit of 1024 (256 on macOS) and a far higher hard limit. A Supervisor holds
//! a few descriptors per loaded thread (harness pipes, shells, sockets, the
//! database), and every harness inherits the limit, so a busy device ran out
//! and new sessions failed with "Too many open files".

/// Enough for thousands of loaded threads. Kept well under the hard limit some
/// systems report (a million or more), because programs that close or scan
/// every possible descriptor get slow with a limit that large.
#[cfg(unix)]
const WANTED: u64 = 65_536;
/// macOS refuses a soft limit above its per-process maximum even when the hard
/// limit says "unlimited"; this value is accepted on every supported release.
#[cfg(unix)]
const FALLBACK: u64 = 10_240;

/// Raises the soft open-file limit towards the hard limit. Never lowers it and
/// never fails: a refused change leaves the limit as it was. Returns the limit
/// in force afterwards.
#[cfg(unix)]
pub fn raise_limit() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid, writable rlimit for the duration of the call.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return None;
    }
    let current = limit.rlim_cur as u64;
    let hard = limit.rlim_max as u64;
    for wanted in [WANTED, FALLBACK] {
        let target = wanted.min(hard);
        if target <= current {
            break;
        }
        let raised = libc::rlimit {
            rlim_cur: target as libc::rlim_t,
            rlim_max: limit.rlim_max,
        };
        // SAFETY: `raised` is a valid rlimit; the call changes only this process.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
            return Some(target);
        }
    }
    Some(current)
}

/// Windows has no per-process descriptor limit of this kind.
#[cfg(not(unix))]
pub fn raise_limit() -> Option<u64> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn the_limit_is_raised_towards_the_hard_limit_and_never_lowered() {
        let read = || {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            assert_eq!(
                unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
                0
            );
            (limit.rlim_cur as u64, limit.rlim_max as u64)
        };
        let (before, hard) = read();
        let reported = super::raise_limit().unwrap();
        let (after, hard_after) = read();
        assert_eq!(reported, after);
        assert_eq!(hard_after, hard);
        assert!(after >= before);
        assert!(after >= before.max(super::FALLBACK.min(hard)));
        assert_eq!(super::raise_limit(), Some(after));
    }
}

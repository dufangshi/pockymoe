//! Bound glibc's per-thread arena retention before Tokio starts its workers.

pub(crate) fn configure() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
        // SAFETY: mallopt is a glibc configuration API. This valid parameter
        // limits future arenas; it does not replace an allocator or invalidate
        // existing allocations. main calls us before starting any workers.
        unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
    }
}

#[cfg(all(test, target_os = "linux", target_env = "gnu"))]
mod tests {
    use std::sync::{Arc, Barrier};

    fn arena_count() -> usize {
        // SAFETY: tmpfile returns an owned FILE; all operations below are on
        // that live stream, and fread writes at most the buffer's length.
        let xml = unsafe {
            let stream = libc::tmpfile();
            assert!(!stream.is_null());
            assert_eq!(libc::malloc_info(0, stream), 0);
            libc::rewind(stream);
            let mut bytes = vec![0_u8; 64 * 1024];
            let len = libc::fread(bytes.as_mut_ptr().cast(), 1, bytes.len(), stream);
            libc::fclose(stream);
            bytes.truncate(len);
            String::from_utf8(bytes).unwrap()
        };
        xml.matches("<heap nr=").count()
    }

    #[test]
    fn arena_limit_and_environment_override() {
        const CHILD: &str = "POCKYMOE_ALLOCATOR_TEST_CHILD";
        if let Ok(case) = std::env::var(CHILD) {
            super::configure();
            let barrier = Arc::new(Barrier::new(25));
            let workers: Vec<_> = (0..24)
                .map(|_| {
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        let buffers: Vec<_> = (0..4096).map(|_| vec![42_u8; 256]).collect();
                        barrier.wait();
                        barrier.wait();
                        std::hint::black_box(buffers);
                    })
                })
                .collect();
            barrier.wait();
            barrier.wait();
            let count = arena_count();
            barrier.wait();
            for worker in workers {
                worker.join().unwrap();
            }
            let expected = if case == "default" { 2 } else { 4 };
            assert_eq!(count, expected, "case {case}: glibc arena count");
            return;
        }
        // Fresh processes make glibc's startup environment and arena count
        // independent of parallel tests and any arenas they already created.
        for case in ["default", "override"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "allocator::tests::arena_limit_and_environment_override",
                    "--nocapture",
                ])
                .env(CHILD, case)
                .env_remove("MALLOC_ARENA_MAX");
            if case == "override" {
                child.env("MALLOC_ARENA_MAX", "4");
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

/// Prevent scan-related metadata operations in the app process from hydrating
/// dataless ancestors. The sidecar also applies a process-wide policy.
pub struct ScanIoGuard {
    #[cfg(target_os = "macos")]
    previous: i32,
    // A thread-scoped policy must never be moved across an await or threads.
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn setiopolicy_np(iotype: i32, scope: i32, policy: i32) -> i32;
    fn getiopolicy_np(iotype: i32, scope: i32) -> i32;
}

impl ScanIoGuard {
    pub fn new() -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            // sys/resource.h: VFS_MATERIALIZE_DATALESS_FILES=3, THREAD=1, OFF=1.
            let previous = unsafe { getiopolicy_np(3, 1) };
            if previous < 0 || unsafe { setiopolicy_np(3, 1, 1) } != 0 {
                return Err(format!(
                    "Cannot disable automatic cloud downloads for local scanning: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self {
                previous,
                _thread_bound: std::marker::PhantomData,
            })
        }
        #[cfg(not(target_os = "macos"))]
        Ok(Self {
            _thread_bound: std::marker::PhantomData,
        })
    }
}

impl Drop for ScanIoGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        unsafe {
            setiopolicy_np(3, 1, self.previous);
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn scan_policy_restores_the_calling_thread() {
        let before = unsafe { super::getiopolicy_np(3, 1) };
        {
            let _guard = super::ScanIoGuard::new().unwrap();
            assert_eq!(unsafe { super::getiopolicy_np(3, 1) }, 1);
        }
        assert_eq!(unsafe { super::getiopolicy_np(3, 1) }, before);
    }
}

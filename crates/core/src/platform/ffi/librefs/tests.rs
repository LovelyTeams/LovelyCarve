use super::*;
use std::sync::Arc;

enum BackingMode {
    Bytes,
    Error,
    Panic,
}

struct TestBacking {
    mode: BackingMode,
    _lifetime: Arc<()>,
}

impl RefsBacking for TestBacking {
    fn pread(&self, offset: u64, buffer: &mut [u8]) -> Result<(), i32> {
        match self.mode {
            BackingMode::Bytes => {
                let start = usize::try_from(offset).map_err(|_| 5)?;
                let end = start.checked_add(buffer.len()).ok_or(5)?;
                let bytes = b"refs".get(start..end).ok_or(5)?;
                buffer.copy_from_slice(bytes);
                Ok(())
            }
            BackingMode::Error => Err(13),
            BackingMode::Panic => panic!("backing read failed"),
        }
    }

    fn size(&self) -> u64 {
        match self.mode {
            BackingMode::Bytes | BackingMode::Error => 4,
            BackingMode::Panic => panic!("backing size failed"),
        }
    }
}

/// Own the callback state just as the native device does, including on test failure.
struct CallbackDevice(*mut c_void);

impl CallbackDevice {
    fn new(mode: BackingMode, lifetime: &Arc<()>) -> Self {
        Self(Box::into_raw(Box::new(BackingState {
            backing: Box::new(TestBacking {
                mode,
                _lifetime: Arc::clone(lifetime),
            }),
        })) as *mut c_void)
    }
}

impl Drop for CallbackDevice {
    fn drop(&mut self) {
        // The constructor transfers one Box to the device; close reclaims it once.
        unsafe { close_trampoline(self.0) }
    }
}

#[test]
fn device_callbacks_read_ranges_validate_pointers_and_release_backing() {
    let lifetime = Arc::new(());
    {
        let device = CallbackDevice::new(BackingMode::Bytes, &lifetime);
        let mut buffer = [0; 2];
        let mut size = 0;
        assert_eq!(Arc::strong_count(&lifetime), 2);
        // These pointers refer to live state and output buffers for the entire call.
        unsafe {
            assert_eq!(
                pread_trampoline(device.0, 1, 2, buffer.as_mut_ptr().cast()),
                0
            );
            assert_eq!(buffer, *b"ef");
            assert_eq!(get_size_trampoline(device.0, &mut size), 0);
            assert_eq!(size, 4);
            assert_eq!(
                pread_trampoline(device.0, 3, 2, buffer.as_mut_ptr().cast()),
                5
            );
            assert_eq!(
                pread_trampoline(ptr::null_mut(), 0, 2, buffer.as_mut_ptr().cast()),
                22
            );
            assert_eq!(pread_trampoline(device.0, 0, 2, ptr::null_mut()), 22);
            assert_eq!(get_size_trampoline(device.0, ptr::null_mut()), 22);
            assert_eq!(get_size_trampoline(ptr::null_mut(), &mut size), 22);
            close_trampoline(ptr::null_mut());
        }
    }
    assert_eq!(Arc::strong_count(&lifetime), 1);
}

#[test]
fn device_callbacks_preserve_errno_and_contain_rust_panics() {
    let lifetime = Arc::new(());
    let error_device = CallbackDevice::new(BackingMode::Error, &lifetime);
    let panic_device = CallbackDevice::new(BackingMode::Panic, &lifetime);
    let mut buffer = [0; 1];
    let mut size = 99;
    // Both device states and output pointers remain alive throughout each call.
    unsafe {
        assert_eq!(
            pread_trampoline(error_device.0, 0, 1, buffer.as_mut_ptr().cast()),
            13
        );
        assert_eq!(
            pread_trampoline(panic_device.0, 0, 1, buffer.as_mut_ptr().cast()),
            5
        );
        assert_eq!(get_size_trampoline(panic_device.0, &mut size), 5);
    }
    assert_eq!(size, 99);
    drop(error_device);
    drop(panic_device);
    assert_eq!(Arc::strong_count(&lifetime), 1);
}

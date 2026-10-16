use applevisor::error::{HypervisorError, Result};
use applevisor_sys::{HV_MEMORY_EXEC, HV_MEMORY_READ, HV_MEMORY_WRITE, hv_vm_map, hv_vm_unmap};
use std::{ffi::c_void, io, ptr};

/// Guest RAM: anonymous host memory mapped into the guest. Unlike memory from
/// `hv_vm_allocate`, macOS takes its pages back when they are discarded.
#[derive(Debug)]
pub struct GuestMemory {
    host_addr: *mut u8,
    guest_addr: u64,
    size: usize,
}

unsafe impl Send for GuestMemory {}
unsafe impl Sync for GuestMemory {}

impl GuestMemory {
    /// Maps `size` bytes of zeroed RAM at `guest_addr`; the VM must exist.
    pub fn new(guest_addr: u64, size: usize) -> Self {
        let host_addr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            )
        };
        assert_ne!(host_addr, libc::MAP_FAILED, "mmap of guest RAM failed: {}", io::Error::last_os_error());
        let result = unsafe { hv_vm_map(host_addr, guest_addr, size, HV_MEMORY_READ | HV_MEMORY_WRITE | HV_MEMORY_EXEC) };
        assert_eq!(result, 0, "hv_vm_map of guest RAM failed: {result:#x}");
        Self {
            host_addr: host_addr.cast(),
            guest_addr,
            size,
        }
    }

    pub fn checked_host_addr(&self, guest_addr: u64, len: usize) -> Result<*mut u8> {
        let end = guest_addr.checked_add(len as u64).ok_or(HypervisorError::BadArgument)?;
        if guest_addr < self.guest_addr || end > self.guest_addr + self.size as u64 {
            return Err(HypervisorError::BadArgument);
        }
        Ok(unsafe { self.host_addr.add((guest_addr - self.guest_addr) as usize) })
    }

    /// Releases the host pages behind guest RAM the guest no longer uses, so they
    /// stop counting toward the VM's memory. A later guest access reads zeroes or
    /// stale data. Only whole host pages inside the range are released.
    pub fn discard(&self, guest_addr: u64, len: usize) -> Result<()> {
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
        let start = guest_addr.next_multiple_of(page);
        let end = (guest_addr + len as u64) / page * page;
        if end <= start {
            return Ok(());
        }
        let host_addr = self.checked_host_addr(start, (end - start) as usize)?;
        let result = unsafe { libc::madvise(host_addr.cast(), (end - start) as usize, libc::MADV_FREE_REUSABLE) };
        assert_eq!(result, 0, "madvise failed: {}", io::Error::last_os_error());
        Ok(())
    }

    pub fn read(&self, guest_addr: u64, data: &mut [u8]) -> Result<()> {
        let host_addr = self.checked_host_addr(guest_addr, data.len())?;
        unsafe { ptr::copy_nonoverlapping(host_addr, data.as_mut_ptr(), data.len()) };
        Ok(())
    }

    pub fn write(&self, guest_addr: u64, data: &[u8]) -> Result<()> {
        let host_addr = self.checked_host_addr(guest_addr, data.len())?;
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), host_addr, data.len()) };
        Ok(())
    }

    fn read_value<T: Copy>(&self, guest_addr: u64) -> Result<T> {
        let host_addr = self.checked_host_addr(guest_addr, size_of::<T>())?;
        Ok(unsafe { ptr::read_unaligned(host_addr.cast::<T>()) })
    }

    pub fn read_u16(&self, guest_addr: u64) -> Result<u16> {
        self.read_value(guest_addr)
    }

    pub fn read_u32(&self, guest_addr: u64) -> Result<u32> {
        self.read_value(guest_addr)
    }

    pub fn read_u64(&self, guest_addr: u64) -> Result<u64> {
        self.read_value(guest_addr)
    }

    pub fn write_u8(&self, guest_addr: u64, value: u8) -> Result<()> {
        self.write(guest_addr, &[value])
    }

    pub fn write_u16(&self, guest_addr: u64, value: u16) -> Result<()> {
        self.write(guest_addr, &value.to_le_bytes())
    }

    pub fn write_u32(&self, guest_addr: u64, value: u32) -> Result<()> {
        self.write(guest_addr, &value.to_le_bytes())
    }
}

impl Drop for GuestMemory {
    fn drop(&mut self) {
        let result = unsafe { hv_vm_unmap(self.guest_addr, self.size) };
        assert_eq!(result, 0, "hv_vm_unmap of guest RAM failed: {result:#x}");
        let result = unsafe { libc::munmap(self.host_addr.cast::<c_void>(), self.size) };
        assert_eq!(result, 0, "munmap of guest RAM failed: {}", io::Error::last_os_error());
    }
}

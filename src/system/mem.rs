//! 进程内存占用查询（Windows PSAPI），用于空闲态内存剖析（目标 < 50MB）。
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

#[derive(Debug, Clone, Copy)]
pub struct MemInfo {
    pub working_set_bytes: u64,
    pub private_bytes: u64,
}

/// 当前进程的 working set（常驻内存）与私有内存（页面文件）占用。
pub fn memory_info() -> MemInfo {
    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut pmc,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ) != 0
        {
            MemInfo {
                working_set_bytes: pmc.WorkingSetSize as u64,
                private_bytes: pmc.PagefileUsage as u64,
            }
        } else {
            MemInfo {
                working_set_bytes: 0,
                private_bytes: 0,
            }
        }
    }
}

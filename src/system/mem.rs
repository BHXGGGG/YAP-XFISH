//! 进程内存占用查询（Windows PSAPI），用于空闲态内存剖析（目标 < 50MB）。
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

#[derive(Debug, Clone, Copy)]
pub struct MemInfo {
    pub working_set_bytes: u64,
    pub private_bytes: u64,
}

/// 指定进程（如 sing-box 子进程）的 working set 字节数，与任务管理器「内存」一致。拿不到返回 0。
pub fn process_working_set(pid: u32) -> u64 {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return 0;
        }
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let ok = GetProcessMemoryInfo(h, &mut pmc, pmc.cb) != 0;
        CloseHandle(h);
        if ok { pmc.WorkingSetSize as u64 } else { 0 }
    }
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

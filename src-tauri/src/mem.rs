//! 进程内存读取。
//!
//! 这不是 Spike 的临时脚手架 —— PRD §5.2 要求内置「性能自检」面板，
//! 展示当前进程内存与各标签内存占用；PRD §5.1 的内存预算
//! （空闲 < 150MB、50 标签 < 400MB）也需要产品自己能测。
//!
//! 所以这是产品能力，Spike 只是第一个消费者。

use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemInfo {
    /// 工作集（Windows: WorkingSetSize / macOS: resident_size），字节
    pub working_set_bytes: u64,
    /// 私有提交（Windows: PrivateUsage），字节。判断真实占用比工作集更可靠。
    pub private_bytes: u64,
    /// 读取是否成功。失败时上面两个值为 0，前端应显示「不可用」而不是「0 MB」。
    pub available: bool,
}

#[cfg(windows)]
pub fn current() -> MemInfo {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    unsafe {
        let mut counters = std::mem::zeroed::<PROCESS_MEMORY_COUNTERS_EX>();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            size,
        );
        if ok == 0 {
            return MemInfo::default();
        }
        MemInfo {
            working_set_bytes: counters.WorkingSetSize as u64,
            private_bytes: counters.PrivateUsage as u64,
            available: true,
        }
    }
}

/// 非 Windows 平台的占位实现。
///
/// 决策 D2 首发只做 Windows + macOS。macOS 侧要用
/// `task_info(mach_task_self(), TASK_BASIC_INFO, ...)` 读 resident_size，
/// 在做 macOS 适配时补上。**返回 available: false 而不是假数据** ——
/// 性能面板显示「不可用」是诚实的，显示 0 会误导。
#[cfg(not(windows))]
pub fn current() -> MemInfo {
    MemInfo::default()
}

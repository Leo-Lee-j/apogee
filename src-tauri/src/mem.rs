//! 进程内存读取。
//!
//! 这不是 Spike 的临时脚手架 —— PRD §5.2 要求内置「性能自检」面板，
//! 展示当前进程内存与各标签内存占用；PRD §5.1 的内存预算
//! （空闲 < 150MB、50 标签 < 400MB）也需要产品自己能测。
//!
//! 所以这是产品能力，Spike 只是第一个消费者。

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMemoryPart {
    pub kind: &'static str,
    pub process_count: u32,
    pub working_set_bytes: u64,
    pub private_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMemInfo {
    pub working_set_bytes: u64,
    pub private_bytes: u64,
    pub available: bool,
    pub process_count: u32,
    pub parts: Vec<AppMemoryPart>,
}

impl Default for AppMemInfo {
    fn default() -> Self {
        Self {
            working_set_bytes: 0,
            private_bytes: 0,
            available: false,
            process_count: 0,
            parts: Vec::new(),
        }
    }
}

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

#[cfg(windows)]
pub fn current_app() -> AppMemInfo {
    use std::collections::{HashMap, HashSet};
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    let root = unsafe { GetCurrentProcessId() };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return AppMemInfo::default();
    }
    let mut parents = HashMap::<u32, (u32, String)>::new();
    unsafe {
        let mut entry = std::mem::zeroed::<PROCESSENTRY32W>();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                parents.insert(
                    entry.th32ProcessID,
                    (
                        entry.th32ParentProcessID,
                        String::from_utf16_lossy(&entry.szExeFile[..end]),
                    ),
                );
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    let mut tree = HashSet::from([root]);
    loop {
        let before = tree.len();
        for (pid, (parent, _)) in &parents {
            if tree.contains(parent) {
                tree.insert(*pid);
            }
        }
        if tree.len() == before {
            break;
        }
    }
    let mut parts = [
        AppMemoryPart {
            kind: "main",
            process_count: 0,
            working_set_bytes: 0,
            private_bytes: 0,
        },
        AppMemoryPart {
            kind: "webview",
            process_count: 0,
            working_set_bytes: 0,
            private_bytes: 0,
        },
        AppMemoryPart {
            kind: "other",
            process_count: 0,
            working_set_bytes: 0,
            private_bytes: 0,
        },
    ];
    for pid in tree {
        let Some((_, name)) = parents.get(&pid) else {
            continue;
        };
        let index = if pid == root {
            0
        } else if name.to_ascii_lowercase().contains("webview")
            || name.to_ascii_lowercase().contains("msedge")
        {
            1
        } else {
            2
        };
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if handle.is_null() {
            continue;
        }
        let mut counters = unsafe { std::mem::zeroed::<PROCESS_MEMORY_COUNTERS_EX>() };
        let ok = unsafe {
            GetProcessMemoryInfo(
                handle,
                &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            )
        };
        unsafe {
            CloseHandle(handle);
        }
        if ok == 0 {
            continue;
        }
        parts[index].process_count += 1;
        parts[index].working_set_bytes += counters.WorkingSetSize as u64;
        parts[index].private_bytes += counters.PrivateUsage as u64;
    }
    let parts = parts
        .into_iter()
        .filter(|part| part.process_count > 0)
        .collect::<Vec<_>>();
    let working = parts.iter().map(|part| part.working_set_bytes).sum();
    let private = parts.iter().map(|part| part.private_bytes).sum();
    AppMemInfo {
        working_set_bytes: working,
        private_bytes: private,
        available: !parts.is_empty(),
        process_count: parts.iter().map(|part| part.process_count).sum(),
        parts,
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

#[cfg(not(windows))]
pub fn current_app() -> AppMemInfo {
    AppMemInfo::default()
}

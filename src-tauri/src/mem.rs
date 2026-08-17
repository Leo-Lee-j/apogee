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
#[repr(C)]
#[derive(Clone, Copy)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

#[cfg(windows)]
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process_handle: *mut std::ffi::c_void,
        process_information_class: u32,
        process_information: *mut std::ffi::c_void,
        process_information_length: u32,
        return_length: *mut u32,
    ) -> i32;
}

/// Chromium 把子进程职责写进命令行的 `--type=`。ToolHelp 只提供 exe 名，
/// 因此读取 PEB 中的 CommandLine；读取失败时保守归到 other。
#[cfg(windows)]
unsafe fn process_command_line(handle: *mut std::ffi::c_void) -> Option<String> {
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;

    const PROCESS_BASIC_INFORMATION: u32 = 0;
    const PEB_PROCESS_PARAMETERS_OFFSET: usize = if cfg!(target_pointer_width = "64") {
        0x20
    } else {
        0x10
    };
    const COMMAND_LINE_OFFSET: usize = if cfg!(target_pointer_width = "64") {
        0x70
    } else {
        0x40
    };

    unsafe fn read<T: Copy>(
        handle: *mut std::ffi::c_void,
        address: *const std::ffi::c_void,
    ) -> Option<T> {
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        if ReadProcessMemory(
            handle,
            address,
            value.as_mut_ptr() as *mut _,
            std::mem::size_of::<T>(),
            std::ptr::null_mut(),
        ) == 0
        {
            return None;
        }
        Some(value.assume_init())
    }

    let mut basic = [0usize; 6];
    if NtQueryInformationProcess(
        handle,
        PROCESS_BASIC_INFORMATION,
        basic.as_mut_ptr() as *mut _,
        std::mem::size_of_val(&basic) as u32,
        std::ptr::null_mut(),
    ) < 0
    {
        return None;
    }
    let peb = basic[1] as *const u8;
    let parameters: *const u8 = read(handle, peb.add(PEB_PROCESS_PARAMETERS_OFFSET) as *const _)?;
    let command: UnicodeString = read(handle, parameters.add(COMMAND_LINE_OFFSET) as *const _)?;
    if command.length == 0 || command.length > 32_768 || command.buffer.is_null() {
        return None;
    }
    let mut chars = vec![0u16; command.length as usize / 2];
    if ReadProcessMemory(
        handle,
        command.buffer as *const _,
        chars.as_mut_ptr() as *mut _,
        command.length as usize,
        std::ptr::null_mut(),
    ) == 0
    {
        return None;
    }
    Some(String::from_utf16_lossy(&chars))
}

#[cfg(windows)]
fn webview_part(command_line: Option<&str>) -> usize {
    match command_line {
        Some(line) if line.contains("--type=renderer") => 2,
        Some(line) if line.contains("--type=gpu-process") => 3,
        Some(line) if line.contains("--type=utility") => 4,
        Some(line) if !line.contains("--type=") => 1,
        _ => 5,
    }
}

/// 进程 ID 会被 Windows 复用，因此父 PID 本身不足以标识父子关系。
/// 返回创建时间（FILETIME 的 100ns tick），供进程树遍历时验证归属。
#[cfg(windows)]
fn process_creation_time(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let ok = unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 {
        return None;
    }
    Some(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
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
    // ToolHelp 仅记录 ParentProcessId。父进程退出、PID 被另一个应用复用后，旧子进程
    // 的 ParentProcessId 会与新进程碰撞；只有创建时间不早于父进程才能算作真实子进程。
    // 无法读取创建时间时宁可漏报，也不能把无关应用的内存归到 Apogee。
    let Some(root_start) = process_creation_time(root) else {
        return AppMemInfo::default();
    };
    let mut starts = HashMap::from([(root, root_start)]);
    let mut tree = HashSet::from([root]);
    loop {
        let mut discovered = Vec::new();
        for (pid, (parent, _)) in &parents {
            if !tree.contains(parent) || tree.contains(pid) {
                continue;
            }
            let Some(&parent_start) = starts.get(parent) else {
                continue;
            };
            let child_start = match starts.get(pid) {
                Some(&started) => started,
                None => match process_creation_time(*pid) {
                    Some(started) => {
                        starts.insert(*pid, started);
                        started
                    }
                    None => continue,
                },
            };
            if child_start >= parent_start {
                discovered.push(*pid);
            }
        }
        let before = tree.len();
        tree.extend(discovered);
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
            kind: "browser",
            process_count: 0,
            working_set_bytes: 0,
            private_bytes: 0,
        },
        AppMemoryPart {
            kind: "renderer",
            process_count: 0,
            working_set_bytes: 0,
            private_bytes: 0,
        },
        AppMemoryPart {
            kind: "gpu",
            process_count: 0,
            working_set_bytes: 0,
            private_bytes: 0,
        },
        AppMemoryPart {
            kind: "utility",
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
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if handle.is_null() {
            continue;
        }
        let index = if pid == root {
            0
        } else if name.to_ascii_lowercase().contains("webview")
            || name.to_ascii_lowercase().contains("msedge")
        {
            // WebView2 的 browser / renderer / GPU / utility 都共享同一个 exe 名，
            // 只能通过命令行的 `--type` 继续区分。
            webview_part(unsafe { process_command_line(handle) }.as_deref())
        } else {
            5
        };
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

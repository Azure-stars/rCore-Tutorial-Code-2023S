//! Process management syscalls

use crate::{
    config::MAX_SYSCALL_NUM,
    mm::page_table::translated_refmut,
    task::{
        change_program_brk, current_task_status_and_time, current_user_token,
        exit_current_and_run_next, get_syscall_time, mmap, munmap, suspend_current_and_run_next,
        TaskStatus,
    },
    timer::get_time_us,
};

/// The time struct
#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    /// 秒级别
    pub sec: usize,
    /// 除去秒级别的时间后剩下的时间
    pub usec: usize,
}

/// The result of Mmap syscall
pub enum MmapResult {
    StartNotAlign,
    PortNotZero,
    PortAllZero,
    PageMapped,
    OotOfMemory,
    PageNotMapped,
}
/// Task information
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct TaskInfo {
    /// Task status in it's life cycle
    /// If add it, then the status field in task inner will be removed
    pub status: TaskStatus,
    /// The numbers of syscall called by task
    pub syscall_times: [u32; MAX_SYSCALL_NUM],
    /// Total running time of task
    pub time: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    *translated_refmut(current_user_token(), _ts) = TimeVal {
        sec: us / 1000000,
        usec: us % 1000000,
    };
    0
}

/// YOUR JOB: Finish sys_task_info to pass testcases
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TaskInfo`] is splitted by two pages ?
pub fn sys_task_info(_ti: *mut TaskInfo) -> isize {
    trace!("kernel: sys_task_info");
    let now_us = get_time_us();
    let (now_status, start_time) = current_task_status_and_time();
    // 以ms为单位
    if now_status != TaskStatus::Running {
        return -1;
    }
    let syscall_times = get_syscall_time();
    let interval = (now_us - start_time) / 1000;
    // unsafe {
    //     *_ti = TaskInfo {
    //         status: now_status,
    //         syscall_times,
    //         time: interval,
    //     }
    // }
    *translated_refmut(current_user_token(), _ti) = TaskInfo {
        status: now_status,
        syscall_times,
        time: interval,
    };
    0
}

/// YOUR JOB: Implement mmap.
pub fn sys_mmap(_start: usize, _len: usize, _port: usize) -> isize {
    trace!("kernel: sys_mmap NOT IMPLEMENTED YET!");
    mmap(_start, _len, _port)
}

/// YOUR JOB: Implement munmap.
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
    trace!("kernel: sys_munmap NOT IMPLEMENTED YET!");
    // let now_result: MmmpResult;
    munmap(_start, _len)
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

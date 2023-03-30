//! Process management syscalls

use crate::{
    config::MAX_SYSCALL_NUM,
    task::{
        current_task, exit_current_and_run_next, get_syscall_time, suspend_current_and_run_next,
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
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    unsafe {
        *ts = TimeVal {
            sec: us / 1000000,
            usec: us % 1000000,
        };
    }
    0
}

/// YOUR JOB: Finish sys_task_info to pass testcases
pub fn sys_task_info(_ti: *mut TaskInfo) -> isize {
    trace!("kernel: sys_task_info");
    let now_us = get_time_us();
    let now_task = current_task();
    // 以ms为单位
    if now_task.status != TaskStatus::Running {
        return -1;
    }
    let syscall_times = get_syscall_time();
    let interval = (now_us - now_task.start_time) / 1000;
    unsafe {
        *_ti = TaskInfo {
            status: now_task.status,
            syscall_times,
            time: interval,
        }
    }
    0
}

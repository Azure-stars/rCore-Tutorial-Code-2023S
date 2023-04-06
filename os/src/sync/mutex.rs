//! Mutex (spin-like and blocking(sleep))

use super::UPSafeCell;
use crate::task::processor::current_task_id;
use crate::task::{block_current_and_run_next, suspend_current_and_run_next};
use crate::task::{current_process, TaskControlBlock};
use crate::task::{current_task, wakeup_task};
use alloc::string::ToString;
use alloc::{collections::VecDeque, sync::Arc};

/// Mutex trait
pub trait Mutex: Sync + Send {
    /// Lock the mutex
    fn lock(&self, mutex_id: usize);
    /// Unlock the mutex
    fn unlock(&self, mutex_id: usize);
}

/// Spinlock Mutex struct
pub struct MutexSpin {
    locked: UPSafeCell<bool>,
}

impl MutexSpin {
    /// Create a new spinlock mutex
    pub fn new() -> Self {
        Self {
            locked: unsafe { UPSafeCell::new(false) },
        }
    }
}

impl Mutex for MutexSpin {
    /// Lock the spinlock mutex
    fn lock(&self, mutex_id: usize) {
        // rid是这个资源在锁资源列表的下标
        trace!("kernel: MutexSpin::lock");
        let task_id = current_task_id();
        loop {
            let mut locked = self.locked.exclusive_access();
            if *locked {
                // 暂时还没有获得资源，不用修改什么的
                drop(locked);
                suspend_current_and_run_next();
                continue;
            } else {
                let process = current_process();
                let mut process_inner = process.inner_exclusive_access();
                let rid = process_inner.get_index_for_resource(mutex_id, "mutex".to_string());
                process_inner.alloc_resource_for_task(task_id, rid);
                // 若是成功拿到资源，就修改自身的Allocation，修改全局空闲资源
                drop(process_inner);
                drop(process);
                *locked = true;
                return;
            }
        }
    }

    fn unlock(&self, mutex_id: usize) {
        trace!("kernel: MutexSpin::unlock");
        let mut locked = self.locked.exclusive_access();
        let task_id = current_task_id();
        let process = current_process();
        let mut process_inner = process.inner_exclusive_access();
        let rid = process_inner.get_index_for_resource(mutex_id, "mutex".to_string());
        process_inner.dealloc_resource_for_task(task_id, rid, false);
        *locked = false;
    }
}

/// Blocking Mutex struct
pub struct MutexBlocking {
    inner: UPSafeCell<MutexBlockingInner>,
}

pub struct MutexBlockingInner {
    locked: bool,
    wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl MutexBlocking {
    /// Create a new blocking mutex
    pub fn new() -> Self {
        trace!("kernel: MutexBlocking::new");
        Self {
            inner: unsafe {
                UPSafeCell::new(MutexBlockingInner {
                    locked: false,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }
}

impl Mutex for MutexBlocking {
    /// lock the blocking mutex
    fn lock(&self, mutex_id: usize) {
        trace!("kernel: MutexBlocking::lock");
        let task_id = current_task_id();
        let mut mutex_inner = self.inner.exclusive_access();
        if mutex_inner.locked {
            mutex_inner.wait_queue.push_back(current_task().unwrap());
            drop(mutex_inner);
            block_current_and_run_next();
        } else {
            let process = current_process();
            let mut process_inner = process.inner_exclusive_access();
            let rid = process_inner.get_index_for_resource(mutex_id, "mutex".to_string());
            process_inner.alloc_resource_for_task(task_id, rid);
            // 拿到资源，添加占用
            drop(process_inner);
            drop(process);
            // 若是成功拿到资源，就修改自身的Allocation，修改全局空闲资源
            mutex_inner.locked = true;
        }
    }

    /// unlock the blocking mutex
    fn unlock(&self, mutex_id: usize) {
        trace!("kernel: MutexBlocking::unlock");
        let mut mutex_inner = self.inner.exclusive_access();
        assert!(mutex_inner.locked);
        // 释放自身资源
        let task_id = current_task_id();
        let process = current_process();
        let mut process_inner = process.inner_exclusive_access();
        let rid = process_inner.get_index_for_resource(mutex_id, "mutex".to_string());

        process_inner.dealloc_resource_for_task(task_id, rid, false);
        drop(process_inner);
        drop(process);
        // 这里的释放资源之所以不传入rid，是因为需要解映射
        if let Some(waking_task) = mutex_inner.wait_queue.pop_front() {
            wakeup_task(waking_task);
        } else {
            mutex_inner.locked = false;
        }
    }
}

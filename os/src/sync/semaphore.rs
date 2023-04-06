//! Semaphore

use crate::sync::UPSafeCell;
use crate::task::processor::current_task_id;
use crate::task::{
    block_current_and_run_next, current_process, current_task, wakeup_task, TaskControlBlock,
};
use alloc::string::ToString;
use alloc::{collections::VecDeque, sync::Arc};

/// semaphore structure
pub struct Semaphore {
    /// semaphore inner
    pub inner: UPSafeCell<SemaphoreInner>,
}

pub struct SemaphoreInner {
    pub count: isize,
    pub wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl Semaphore {
    /// Create a new semaphore
    pub fn new(res_count: usize) -> Self {
        trace!("kernel: Semaphore::new");
        Self {
            inner: unsafe {
                UPSafeCell::new(SemaphoreInner {
                    count: res_count as isize,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }

    /// up operation of semaphore
    pub fn up(&self, sem_id: usize) {
        trace!("kernel: Semaphore::up");
        let mut inner = self.inner.exclusive_access();
        inner.count += 1;
        // 相当于释放资源
        let tid = current_task_id();
        let process = current_process();
        let mut process_inner = process.inner_exclusive_access();
        let rid = process_inner.get_index_for_resource(sem_id, "sem".to_string());
        process_inner.dealloc_resource_for_task(tid, rid, false);
        if inner.count <= 0 {
            if let Some(task) = inner.wait_queue.pop_front() {
                let task_inner = task.inner_exclusive_access();
                let tid = task_inner.res.as_ref().unwrap().tid;
                drop(task_inner);
                // 此时被堵塞的这个任务才真正拿到了资源
                let rid = process_inner.get_index_for_resource(sem_id, "sem".to_string());
                process_inner.alloc_resource_for_task(tid, rid);
                drop(process_inner);
                drop(process);
                wakeup_task(task);
            }
        }
    }

    /// down operation of semaphore
    pub fn down(&self, sem_id: usize) {
        trace!("kernel: Semaphore::down");
        let mut inner = self.inner.exclusive_access();
        inner.count -= 1;
        if inner.count < 0 {
            // 注意此时是还在等待状态，所以不能修改为alloc
            inner.wait_queue.push_back(current_task().unwrap());
            drop(inner);
            block_current_and_run_next();
        } else {
            let task_id = current_task_id();
            // 非堵塞情况下，任务才真正拿到了资源
            let process = current_process();
            let mut process_inner = process.inner_exclusive_access();
            let rid = process_inner.get_index_for_resource(sem_id, "sem".to_string());
            process_inner.alloc_resource_for_task(task_id, rid);
            drop(process_inner);
        }
    }
}

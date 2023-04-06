//! Conditian variable

use crate::sync::{Mutex, UPSafeCell};
use crate::task::processor::current_task_id;
use crate::task::{
    block_current_and_run_next, current_process, current_task, wakeup_task, TaskControlBlock,
};
use alloc::string::ToString;
use alloc::{collections::VecDeque, sync::Arc};

/// Condition variable structure
pub struct Condvar {
    /// Condition variable inner
    pub inner: UPSafeCell<CondvarInner>,
}

pub struct CondvarInner {
    pub wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl Condvar {
    /// Create a new condition variable
    pub fn new() -> Self {
        trace!("kernel: Condvar::new");
        Self {
            inner: unsafe {
                UPSafeCell::new(CondvarInner {
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }

    /// Signal a task waiting on the condition variable
    pub fn signal(&self) {
        let mut inner = self.inner.exclusive_access();
        if let Some(task) = inner.wait_queue.pop_front() {
            wakeup_task(task);
        }
    }

    /// blocking current task, let it wait on the condition variable
    pub fn wait(&self, mutex: Arc<dyn Mutex>, mutex_id: usize) -> isize {
        trace!("kernel: Condvar::wait_with_mutex");
        mutex.unlock(mutex_id);
        let mut inner = self.inner.exclusive_access();
        inner.wait_queue.push_back(current_task().unwrap());
        drop(inner);
        block_current_and_run_next();
        let tid = current_task_id();
        let process = current_process();
        let mut process_inner = process.inner_exclusive_access();
        let rid = process_inner.get_index_for_resource(mutex_id, "mutex".to_string());
        // 先修改自己的need，之后查看是否满足要求
        process_inner.add_need(tid, rid);
        if process_inner.deadlock_detect == true {
            if process_inner.detect_for_resourece_request() == false {
                return -0xDEAD;
            }
        }
        drop(process_inner);
        mutex.lock(mutex_id);
        0
    }
}

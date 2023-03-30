//! Types related to task management

use super::TaskContext;

/// The task control block (TCB) of a task.
#[derive(Clone, Copy)]
pub struct TaskControlBlock {
    /// The task context
    pub task_cx: TaskContext,
    /// The task info in it's lifecycle
    pub status: TaskStatus,
    /// The start time of the task,which is ms
    pub start_time: usize,
}

/// The status of a task
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Exited,
}

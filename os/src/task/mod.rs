//! Task management implementation
//!
//! Everything about task management, like starting and switching tasks is
//! implemented here.
//!
//! A single global instance of [`TaskManager`] called `TASK_MANAGER` controls
//! all the tasks in the whole operating system.
//!
//! A single global instance of [`Processor`] called `PROCESSOR` monitors running
//! task(s) for each core.
//!
//! A single global instance of `PID_ALLOCATOR` allocates pid for user apps.
//!
//! Be careful when you see `__switch` ASM function in `switch.S`. Control flow around this function
//! might not be what you expect.
mod context;
mod id;
mod manager;
pub mod processor;
mod switch;
#[allow(clippy::module_inception)]
#[allow(rustdoc::private_intra_doc_links)]
mod task;

use crate::fs::{open_file, OpenFlags};
use alloc::sync::Arc;
pub use context::TaskContext;
use lazy_static::*;
pub use manager::{fetch_task, TaskManager};
use switch::__switch;
pub use task::{TaskControlBlock, TaskStatus};

pub use id::{kstack_alloc, pid_alloc, KernelStack, PidHandle};
pub use manager::add_task;
pub use processor::{
    current_task, current_trap_cx, current_user_token, run_tasks, schedule, take_current_task,
    Processor,
};

use crate::loader::get_app_data_by_name;
/// Suspend the current 'Running' task and run the next task in task list.
pub fn suspend_current_and_run_next() {
    // There must be an application running.
    let task = take_current_task().unwrap();

    // ---- access current TCB exclusively
    let mut task_inner = task.inner_exclusive_access();
    let task_cx_ptr = &mut task_inner.task_cx as *mut TaskContext;
    // Change status to Ready
    task_inner.task_status = TaskStatus::Ready;
    drop(task_inner);
    // ---- release current PCB

    // push back to ready queue.
    add_task(task);
    // jump to scheduling cycle
    schedule(task_cx_ptr);
}

/// pid of usertests app in make run TEST=1
pub const IDLE_PID: usize = 0;

/// Exit the current 'Running' task and run the next task in task list.
pub fn exit_current_and_run_next(exit_code: i32) {
    // take from Processor
    let task = take_current_task().unwrap();

    let pid = task.getpid();
    if pid == IDLE_PID {
        println!(
            "[kernel] Idle process exit with exit_code {} ...",
            exit_code
        );
        panic!("All applications completed!");
    }

    // **** access current TCB exclusively
    let mut inner = task.inner_exclusive_access();
    // Change status to Zombie
    inner.task_status = TaskStatus::Zombie;
    // Record exit code
    inner.exit_code = exit_code;
    // do not move to its parent but under initproc

    // ++++++ access initproc TCB exclusively
    {
        let mut initproc_inner = INITPROC.inner_exclusive_access();
        for child in inner.children.iter() {
            child.inner_exclusive_access().parent = Some(Arc::downgrade(&INITPROC));
            initproc_inner.children.push(child.clone());
        }
    }
    // ++++++ release parent PCB

    inner.children.clear();
    // deallocate user space
    inner.memory_set.recycle_data_pages();
    // drop file descriptors
    inner.fd_table.clear();
    drop(inner);
    // **** release current PCB
    // drop task manually to maintain rc correctly
    drop(task);
    // we do not have to save task context
    let mut _unused = TaskContext::zero_init();
    schedule(&mut _unused as *mut _);
}

lazy_static! {
    /// Creation of initial process
    ///
    /// the name "initproc" may be changed to any other app name like "usertests",
    /// but we have user_shell, so we don't need to change it.
    pub static ref INITPROC: Arc<TaskControlBlock> = Arc::new({
        let inode = open_file("ch6b_initproc", OpenFlags::RDONLY).unwrap();
        let v = inode.read_all();
        TaskControlBlock::new(v.as_slice())
    });
}

///Add init process to the manager
pub fn add_initproc() {
    add_task(INITPROC.clone());
}
// /// Inner of Task Manager
// pub struct TaskManagerInner {
//     /// syscall time
//     syscall_times: Vec<BTreeMap<usize, u32>>,
//     /// task list
//     tasks: Vec<TaskControlBlock>,
//     /// id of current `Running` task
//     current_task: usize,
// }

// lazy_static! {
//     /// a `TaskManager` global instance through lazy_static!
//     pub static ref TASK_MANAGER: TaskManager = {
//         println!("init TASK_MANAGER");
//         let num_app = get_num_app();
//         println!("num_app = {}", num_app);
//         let mut tasks: Vec<TaskControlBlock> = Vec::new();
//         let mut syscall_times:Vec<BTreeMap<usize, u32>> = Vec::new();
//         for i in 0..num_app {
//             syscall_times.insert(syscall_times.len(), BTreeMap::new());
//             tasks.push(TaskControlBlock::new(get_app_data(i), i));
//         }
//         TaskManager {
//             num_app,
//             inner: unsafe {
//                 UPSafeCell::new(TaskManagerInner {
//                     syscall_times,
//                     tasks,
//                     current_task: 0,
//                 })
//             },
//         }
//     };
// }

// impl TaskManager {
//     /// Run the first task in task list.
//     ///
//     /// Generally, the first task in task list is an idle task (we call it zero process later).
//     /// But in ch4, we load apps statically, so the first task is a real app.
//     fn run_first_task(&self) -> ! {
//         let mut inner = self.inner.exclusive_access();
//         let task0 = &mut inner.tasks[0];
//         task0.task_status = TaskStatus::Running;
//         // 一定是这个任务刚刚启动
//         task0.start_time = get_time_us();
//         let next_task_cx_ptr = &task0.task_cx as *const TaskContext;
//         drop(inner);
//         let mut _unused = TaskContext::zero_init();
//         // before this, we should drop local variables that must be dropped manually
//         unsafe {
//             __switch(&mut _unused as *mut _, next_task_cx_ptr);
//         }
//         panic!("unreachable in run_first_task!");
//     }

//     /// Change the status of current `Running` task into `Ready`.
//     fn mark_current_suspended(&self) {
//         let mut inner = self.inner.exclusive_access();
//         let current = inner.current_task;
//         inner.tasks[current].task_status = TaskStatus::Ready;
//     }

//     /// Change the status of current `Running` task into `Exited`.
//     fn mark_current_exited(&self) {
//         let mut inner = self.inner.exclusive_access();
//         let current = inner.current_task;
//         inner.tasks[current].task_status = TaskStatus::Exited;
//     }

//     /// Find next task to run and return task id.
//     ///
//     /// In this case, we only return the first `Ready` task in task list.
//     fn find_next_task(&self) -> Option<usize> {
//         let inner = self.inner.exclusive_access();
//         let current = inner.current_task;
//         (current + 1..current + self.num_app + 1)
//             .map(|id| id % self.num_app)
//             .find(|id| inner.tasks[*id].task_status == TaskStatus::Ready)
//     }

//     /// Get the current 'Running' task's token.
//     fn get_current_token(&self) -> usize {
//         let inner = self.inner.exclusive_access();
//         inner.tasks[inner.current_task].get_user_token()
//     }

//     /// Get the current 'Running' task's trap contexts.
//     fn get_current_trap_cx(&self) -> &'static mut TrapContext {
//         let inner = self.inner.exclusive_access();
//         inner.tasks[inner.current_task].get_trap_cx()
//     }

//     /// Change the current 'Running' task's program break
//     pub fn change_current_program_brk(&self, size: i32) -> Option<usize> {
//         let mut inner = self.inner.exclusive_access();
//         let cur = inner.current_task;
//         inner.tasks[cur].change_program_brk(size)
//     }
//     /// Get the memory mapped
//     pub fn mmap(&self, start: usize, len: usize, port: usize) -> isize {
//         let mut inner = self.inner.exclusive_access();
//         let cur = inner.current_task;
//         if inner.tasks[cur].mmap(start, len, port).is_err() {
//             return -1;
//         }
//         0
//     }

//     /// Get the memory unmapped
//     pub fn munmap(&self, start: usize, len: usize) -> isize {
//         let mut inner = self.inner.exclusive_access();
//         let cur = inner.current_task;
//         if inner.tasks[cur].munmap(start, len).is_err() {
//             return -1;
//         }
//         0
//     }

//     /// Switch current `Running` task to the task we have found,
//     /// or there is no `Ready` task and we can exit with all applications completed
//     fn run_next_task(&self) {
//         if let Some(next) = self.find_next_task() {
//             let mut inner = self.inner.exclusive_access();
//             let current = inner.current_task;
//             if inner.tasks[next].start_time == 0 {
//                 inner.tasks[next].start_time = get_time_us();
//             }
//             inner.tasks[next].task_status = TaskStatus::Running;
//             inner.current_task = next;
//             let current_task_cx_ptr = &mut inner.tasks[current].task_cx as *mut TaskContext;
//             let next_task_cx_ptr = &inner.tasks[next].task_cx as *const TaskContext;
//             drop(inner);
//             // before this, we should drop local variables that must be dropped manually
//             unsafe {
//                 __switch(current_task_cx_ptr, next_task_cx_ptr);
//             }
//             // go back to user mode
//         } else {
//             panic!("All applications completed!");
//         }
//     }
//     fn set_syscall(&self, syscall_id: usize) -> isize {
//         if syscall_id >= MAX_SYSCALL_NUM {
//             return -1;
//         }
//         let mut inner = self.inner.exclusive_access();
//         let task_id = inner.current_task;
//         let count = inner.syscall_times[task_id].entry(syscall_id).or_insert(0);
//         *count += 1;
//         0
//     }
//     fn current_task_status_and_time(&self) -> (TaskStatus, usize) {
//         let inner = self.inner.exclusive_access();
//         let task_id = inner.current_task;
//         let task = &inner.tasks[task_id];
//         (task.task_status, task.start_time)
//     }
//     fn get_syscall_time(&self) -> [u32; MAX_SYSCALL_NUM] {
//         let inner = self.inner.exclusive_access();
//         let task_id = inner.current_task;
//         let mut syscall_time = [0; MAX_SYSCALL_NUM];
//         for (key, val) in inner.syscall_times[task_id].iter() {
//             syscall_time[*key] = *val;
//         }
//         syscall_time
//     }
// }

// /// Change the task info
// /// if success, return 0
// /// else return -1
// pub fn set_syscall(syscall_id: usize) -> isize {
//     TASK_MANAGER.set_syscall(syscall_id)
// }

// /// Get the current task info
// pub fn current_task_status_and_time() -> (TaskStatus, usize) {
//     TASK_MANAGER.current_task_status_and_time()
// }

// /// Get the syscall_time of current task
// pub fn get_syscall_time() -> [u32; MAX_SYSCALL_NUM] {
//     TASK_MANAGER.get_syscall_time()
// }

// ///Add init process to the manager
// pub fn add_initproc() {
//     add_task(INITPROC.clone());
// }

// /// Get the memory mapped
// pub fn mmap(start: usize, len: usize, port: usize) -> isize {
//     TASK_MANAGER.mmap(start, len, port)
// }

// /// Get the memory unmapped
// pub fn munmap(start: usize, len: usize) -> isize {
//     TASK_MANAGER.munmap(start, len)
// }

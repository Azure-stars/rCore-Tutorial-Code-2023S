//! Types related to task management & Functions for completely changing TCB
use core::cell::RefMut;
use core::cmp::Ordering;

use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use super::TaskContext;
use super::{kstack_alloc, pid_alloc, KernelStack, PidHandle};
use crate::config::{BIGSTRIDE, MAX_SYSCALL_NUM, TRAP_CONTEXT_BASE};
use crate::mm::{MapPermission, MemorySet, PhysPageNum, VirtAddr, VirtPageNum, KERNEL_SPACE};
use crate::sync::UPSafeCell;
use crate::syscall::process::MmapResult;
use crate::trap::{trap_handler, TrapContext};
/// The task control block (TCB) of a task.
pub struct TaskControlBlock {
    // Immutable
    /// Process identifier
    pub pid: PidHandle,

    /// Kernel stack corresponding to PID
    pub kernel_stack: KernelStack,

    /// Mutable
    inner: UPSafeCell<TaskControlBlockInner>,
}

impl Ord for TaskControlBlock {
    fn max(self, other: Self) -> Self
    where
        Self: Sized,
        Self: ~const core::marker::Destruct,
    {
        let pre_inner_stride = self.inner.exclusive_access().stride;
        let suc_inner_stride = other.inner.exclusive_access().stride;
        if pre_inner_stride > suc_inner_stride {
            return self;
        } else {
            return other;
        }
    }
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        let pre_inner = self.inner.exclusive_access();
        let suc_inner = other.inner.exclusive_access();
        if pre_inner.stride < suc_inner.stride {
            return Ordering::Less;
        } else if pre_inner.stride > suc_inner.stride {
            return Ordering::Greater;
        } else {
            return Ordering::Equal;
        }
    }
    fn min(self, other: Self) -> Self
    where
        Self: Sized,
        Self: ~const core::marker::Destruct,
    {
        let pre_inner_stride = self.inner.exclusive_access().stride;
        let suc_inner_stride = other.inner.exclusive_access().stride;
        if pre_inner_stride < suc_inner_stride {
            return self;
        } else {
            return other;
        }
    }
    fn clamp(self, min: Self, max: Self) -> Self
    where
        Self: Sized,
        Self: ~const core::marker::Destruct,
        Self: ~const PartialOrd,
    {
        let pre_inner_stride = self.inner.exclusive_access().stride;
        let min_inner_stride = min.inner.exclusive_access().stride;
        let max_inner_stride = max.inner.exclusive_access().stride;
        if pre_inner_stride < min_inner_stride {
            return min;
        } else if pre_inner_stride > max_inner_stride {
            return max;
        } else {
            return self;
        }
    }
}

impl PartialOrd for TaskControlBlock {
    fn ge(&self, other: &Self) -> bool {
        let pre_inner = self.inner.exclusive_access();
        let suc_inner = other.inner.exclusive_access();
        pre_inner.stride >= suc_inner.stride
    }
    fn gt(&self, other: &Self) -> bool {
        let pre_inner = self.inner.exclusive_access();
        let suc_inner = other.inner.exclusive_access();
        pre_inner.stride > suc_inner.stride
    }
    fn le(&self, other: &Self) -> bool {
        let pre_inner = self.inner.exclusive_access();
        let suc_inner = other.inner.exclusive_access();
        pre_inner.stride <= suc_inner.stride
    }
    fn lt(&self, other: &Self) -> bool {
        let pre_inner = self.inner.exclusive_access();
        let suc_inner = other.inner.exclusive_access();
        pre_inner.stride < suc_inner.stride
    }
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let pre_inner = self.inner.exclusive_access();
        let suc_inner = other.inner.exclusive_access();
        if pre_inner.stride < suc_inner.stride {
            return Some(Ordering::Less);
        } else if pre_inner.stride > suc_inner.stride {
            return Some(Ordering::Greater);
        } else {
            return Some(Ordering::Equal);
        }
    }
}

impl PartialEq for TaskControlBlock {
    fn eq(&self, other: &Self) -> bool {
        let pre_inner = self.inner.exclusive_access();
        let suc_inner = other.inner.exclusive_access();
        pre_inner.stride == suc_inner.stride
    }
    fn ne(&self, other: &Self) -> bool {
        let pre_inner = self.inner.exclusive_access();
        let suc_inner = other.inner.exclusive_access();
        pre_inner.stride != suc_inner.stride
    }
}

impl Eq for TaskControlBlock {}

impl TaskControlBlock {
    /// Get the mutable reference of the inner TCB
    pub fn inner_exclusive_access(&self) -> RefMut<'_, TaskControlBlockInner> {
        self.inner.exclusive_access()
    }
    /// Get the address of app's page table
    pub fn get_user_token(&self) -> usize {
        let inner = self.inner_exclusive_access();
        inner.memory_set.token()
    }
}

pub struct TaskControlBlockInner {
    /// The physical page number of the frame where the trap context is placed
    pub trap_cx_ppn: PhysPageNum,

    /// Application data can only appear in areas
    /// where the application address space is lower than base_size
    pub base_size: usize,

    /// Save task context
    pub task_cx: TaskContext,

    /// Maintain the execution status of the current process
    pub task_status: TaskStatus,

    /// Application address space
    pub memory_set: MemorySet,

    /// Parent process of the current process.
    /// Weak will not affect the reference count of the parent
    pub parent: Option<Weak<TaskControlBlock>>,

    /// A vector containing TCBs of all child processes of the current process
    pub children: Vec<Arc<TaskControlBlock>>,

    /// It is set when active exit or execution error occurs
    pub exit_code: i32,

    /// Heap bottom
    pub heap_bottom: usize,

    /// Program break
    pub program_brk: usize,

    /// Start time
    pub start_time: usize,

    /// syscall_time
    pub syscall_time: BTreeMap<usize, u32>,

    // stride for alloc
    pub stride: usize,

    // priority
    pub pass: usize,
}

impl TaskControlBlockInner {
    /// get the trap context
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.trap_cx_ppn.get_mut()
    }
    /// get the user token
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    fn get_status(&self) -> TaskStatus {
        self.task_status
    }
    pub fn is_zombie(&self) -> bool {
        self.get_status() == TaskStatus::Zombie
    }
}

impl TaskControlBlock {
    /// Create a new process
    ///
    /// At present, it is only used for the creation of initproc
    pub fn new(elf_data: &[u8]) -> Self {
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        // alloc a pid and a kernel stack in kernel space
        let pid_handle = pid_alloc();
        let kernel_stack = kstack_alloc();
        let kernel_stack_top = kernel_stack.get_top();
        // push a task context which goes to trap_return to the top of kernel stack
        let task_control_block = Self {
            pid: pid_handle,
            kernel_stack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: user_sp,
                    task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                    task_status: TaskStatus::Ready,
                    memory_set,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    heap_bottom: user_sp,
                    program_brk: user_sp,
                    start_time: 0,
                    syscall_time: BTreeMap::new(),
                    stride: 0,
                    pass: BIGSTRIDE / 16,
                })
            },
        };
        // prepare TrapContext in user space
        let trap_cx = task_control_block.inner_exclusive_access().get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            kernel_stack_top,
            trap_handler as usize,
        );
        task_control_block
    }

    /// Load a new elf to replace the original application address space and start execution
    pub fn exec(&self, elf_data: &[u8]) {
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();

        // **** access current TCB exclusively
        let mut inner = self.inner_exclusive_access();
        // substitute memory_set
        inner.memory_set = memory_set;
        // update trap_cx ppn
        inner.trap_cx_ppn = trap_cx_ppn;
        // initialize base_size
        inner.base_size = user_sp;
        // initialize trap_cx
        let trap_cx = inner.get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            self.kernel_stack.get_top(),
            trap_handler as usize,
        );
        // **** release inner automatically
    }

    /// parent process fork the child process
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        // ---- access parent PCB exclusively
        let mut parent_inner = self.inner_exclusive_access();
        // copy user space(include trap context)
        let memory_set = MemorySet::from_existed_user(&parent_inner.memory_set);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        // alloc a pid and a kernel stack in kernel space
        let pid_handle = pid_alloc();
        let kernel_stack = kstack_alloc();
        let kernel_stack_top = kernel_stack.get_top();
        let task_control_block = Arc::new(TaskControlBlock {
            pid: pid_handle,
            kernel_stack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: parent_inner.base_size,
                    task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                    task_status: TaskStatus::Ready,
                    memory_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    heap_bottom: parent_inner.heap_bottom,
                    program_brk: parent_inner.program_brk,
                    start_time: 0, // 新任务的时间从0开始
                    syscall_time: BTreeMap::new(),
                    stride: 0,
                    pass: BIGSTRIDE / 16, // 认为子进程的优先级与父进程无关
                })
            },
        });
        // add child
        parent_inner.children.push(task_control_block.clone());
        // modify kernel_sp in trap_cx
        // **** access child PCB exclusively
        let trap_cx = task_control_block.inner_exclusive_access().get_trap_cx();
        trap_cx.kernel_sp = kernel_stack_top;
        // return
        task_control_block
        // **** release child PCB
        // ---- release parent PCB
    }

    /// 创建一个新的进程并且执行目标程序
    pub fn spawn(self: &Arc<Self>, elf_data: &[u8]) -> Arc<Self> {
        let mut parent_inner = self.inner_exclusive_access();
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();

        // **** access current TCB exclusively
        // 分配pid和内核栈
        let pid_handle = pid_alloc();
        let kernel_stack = kstack_alloc();
        let kernel_stack_top = kernel_stack.get_top();
        let task_control_block = Arc::new(TaskControlBlock {
            pid: pid_handle,
            kernel_stack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: user_sp,
                    task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                    task_status: TaskStatus::Ready,
                    memory_set,
                    parent: Some(Arc::downgrade(self)), // 设置父进程
                    children: Vec::new(),
                    exit_code: 0,
                    heap_bottom: user_sp,
                    program_brk: user_sp, // 当前分配的堆内存
                    start_time: 0,        // 新任务的时间从0开始
                    syscall_time: BTreeMap::new(),
                    stride: 0,
                    pass: BIGSTRIDE / 16,
                })
            },
        });
        // add child
        parent_inner.children.push(task_control_block.clone());
        let trap_cx = task_control_block.inner_exclusive_access().get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            kernel_stack_top,
            trap_handler as usize,
        );
        task_control_block
    }
    /// get pid of process
    pub fn getpid(&self) -> usize {
        self.pid.0
    }

    /// set the priority
    pub fn sys_set_priority(&self, prio: isize) {
        let mut inner = self.inner.exclusive_access();
        inner.pass = BIGSTRIDE / (prio as usize);
    }

    /// change the location of the program break. return None if failed.
    pub fn change_program_brk(&self, size: i32) -> Option<usize> {
        let mut inner = self.inner_exclusive_access();
        let heap_bottom = inner.heap_bottom;
        let old_break = inner.program_brk;
        let new_brk = inner.program_brk as isize + size as isize;
        if new_brk < heap_bottom as isize {
            return None;
        }
        let result = if size < 0 {
            inner
                .memory_set
                .shrink_to(VirtAddr(heap_bottom), VirtAddr(new_brk as usize))
        } else {
            inner
                .memory_set
                .append_to(VirtAddr(heap_bottom), VirtAddr(new_brk as usize))
        };
        if result {
            inner.program_brk = new_brk as usize;
            Some(old_break)
        } else {
            None
        }
    }
    /// 应当为起点start新建一个逻辑段
    /// 首先查找[start, start + len) 是否在某一个逻辑段中
    /// 如果分配失败，返回虚拟页对应的实际编码
    pub fn mmap(&self, start: usize, len: usize, port: usize) -> Result<(), MmapResult> {
        let start_va = VirtAddr::from(start);
        if start_va.page_offset() != 0 {
            return Err(MmapResult::StartNotAlign);
        }
        let end_va = VirtAddr::from(start + len);
        let mut inner = self.inner.exclusive_access();
        if let Some(_) = inner.memory_set.areas.iter().find(|area| {
            area.vpn_range.get_start() < end_va.ceil()
                && area.vpn_range.get_end() > start_va.floor()
        }) {
            // 注意ceil和大小关系
            // 已经分配了物理页帧
            return Err(MmapResult::PageMapped);
        }
        let mut permission = MapPermission::U;
        if port & 0x1 != 0 {
            permission |= MapPermission::R;
        }
        if port & 0x2 != 0 {
            permission |= MapPermission::W;
        }
        if port & 0x4 != 0 {
            permission |= MapPermission::X;
        }
        if port & !0x7 != 0 {
            return Err(MmapResult::PortNotZero);
        }
        if port & 0x7 == 0 {
            return Err(MmapResult::PortAllZero);
        }
        if let Err(x) = inner
            .memory_set
            .insert_framed_area(start_va, end_va, permission)
        {
            if x == VirtPageNum(0) {
                return Err(MmapResult::OotOfMemory);
            }
            return Err(MmapResult::PageMapped);
        }
        Ok(())
    }

    /// 将[start, start+len)的部分解映射
    /// 可能会生成两个新的area
    pub fn munmap(&self, start: usize, len: usize) -> Result<(), MmapResult> {
        let start_va = VirtAddr::from(start);
        if start_va.page_offset() != 0 {
            return Err(MmapResult::StartNotAlign);
        }
        let end_va = VirtAddr::from(start + len);
        let mut inner = self.inner.exclusive_access();
        inner.memory_set.split(start_va, end_va)
    }

    /// record the syscall time
    pub fn set_syscall(&self, syscall_id: usize) {
        let mut inner = self.inner.exclusive_access();
        let count = inner.syscall_time.entry(syscall_id).or_insert(0);
        *count += 1;
    }

    /// get the syscall time
    pub fn get_syscall_time(&self) -> [u32; MAX_SYSCALL_NUM] {
        let inner = self.inner.exclusive_access();
        let mut data = [0 as u32; MAX_SYSCALL_NUM];
        for (key, val) in inner.syscall_time.iter() {
            data[*key] = *val;
        }
        data
    }

    /// Get the status and time
    pub fn get_current_status_and_time(&self) -> (TaskStatus, usize) {
        let inner = self.inner.exclusive_access();
        (inner.task_status, inner.start_time)
    }
}

#[derive(Copy, Clone, PartialEq)]
/// task status: UnInit, Ready, Running, Exited
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Zombie,
}

//! Types related to task management
use super::TaskContext;
use crate::config::TRAP_CONTEXT_BASE;
use crate::mm::{
    kernel_stack_position, MapPermission, MemorySet, PhysPageNum, VirtAddr, VirtPageNum,
    KERNEL_SPACE,
};
use crate::syscall::process::MmapResult;
use crate::trap::{trap_handler, TrapContext};
/// The task control block (TCB) of a task.
pub struct TaskControlBlock {
    /// Save task context
    pub task_cx: TaskContext,

    /// Maintain the execution status of the current process
    pub task_status: TaskStatus,

    /// Application address space
    pub memory_set: MemorySet,

    /// The phys page number of trap context
    pub trap_cx_ppn: PhysPageNum,

    /// The size(top addr) of program which is loaded from elf file
    pub base_size: usize,

    /// Heap bottom
    pub heap_bottom: usize,

    /// Program break
    pub program_brk: usize,

    /// Start time
    pub start_time: usize,
}

impl TaskControlBlock {
    /// get the trap context
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.trap_cx_ppn.get_mut()
    }
    /// get the user token
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    /// Based on the elf info in program, build the contents of task in a new address space
    pub fn new(elf_data: &[u8], app_id: usize) -> Self {
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        let task_status = TaskStatus::Ready;
        // map a kernel-stack in kernel space
        let (kernel_stack_bottom, kernel_stack_top) = kernel_stack_position(app_id);
        KERNEL_SPACE
            .exclusive_access()
            .insert_framed_area(
                kernel_stack_bottom.into(),
                kernel_stack_top.into(),
                MapPermission::R | MapPermission::W,
            )
            .unwrap();
        let task_control_block = Self {
            task_status,
            task_cx: TaskContext::goto_trap_return(kernel_stack_top),
            memory_set,
            trap_cx_ppn,
            base_size: user_sp,
            heap_bottom: user_sp,
            program_brk: user_sp,
            start_time: 0,
        };
        // prepare TrapContext in user space
        let trap_cx = task_control_block.get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            kernel_stack_top,
            trap_handler as usize,
        );
        task_control_block
    }
    /// change the location of the program break. return None if failed.
    pub fn change_program_brk(&mut self, size: i32) -> Option<usize> {
        let old_break = self.program_brk;
        let new_brk = self.program_brk as isize + size as isize;
        if new_brk < self.heap_bottom as isize {
            return None;
        }
        let result = if size < 0 {
            self.memory_set
                .shrink_to(VirtAddr(self.heap_bottom), VirtAddr(new_brk as usize))
        } else {
            self.memory_set
                .append_to(VirtAddr(self.heap_bottom), VirtAddr(new_brk as usize))
        };
        if result {
            self.program_brk = new_brk as usize;
            Some(old_break)
        } else {
            None
        }
    }
    /// 应当为起点start新建一个逻辑段
    /// 首先查找[start, start + len) 是否在某一个逻辑段中
    /// 如果分配失败，返回虚拟页对应的实际编码
    pub fn mmap(&mut self, start: usize, len: usize, port: usize) -> Result<(), MmapResult> {
        let start_va = VirtAddr::from(start);
        if start_va.page_offset() != 0 {
            return Err(MmapResult::StartNotAlign);
        }
        let end_va = VirtAddr::from(start + len);
        if let Some(_) = self.memory_set.areas.iter().find(|area| {
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
        if let Err(x) = self
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
    pub fn munmap(&mut self, start: usize, len: usize) -> Result<(), MmapResult> {
        let start_va = VirtAddr::from(start);
        if start_va.page_offset() != 0 {
            return Err(MmapResult::StartNotAlign);
        }
        let end_va = VirtAddr::from(start + len);
        self.memory_set.split(start_va, end_va)
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
    Exited,
}

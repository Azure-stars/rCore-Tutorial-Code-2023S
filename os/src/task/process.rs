//! Implementation of  [`ProcessControlBlock`]

use super::id::RecycleAllocator;
use super::manager::insert_into_pid2process;
use super::TaskControlBlock;
use super::{add_task, SignalFlags};
use super::{pid_alloc, PidHandle};
use crate::fs::{File, Stdin, Stdout};
use crate::mm::{translated_refmut, MemorySet, KERNEL_SPACE};
use crate::sync::{Condvar, Mutex, Semaphore, UPSafeCell};
use crate::trap::{trap_handler, TrapContext};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefMut;
/// Process Control Block
pub struct ProcessControlBlock {
    /// immutable
    pub pid: PidHandle,
    /// mutable
    inner: UPSafeCell<ProcessControlBlockInner>,
}

/// Inner of Process Control Block
pub struct ProcessControlBlockInner {
    /// is zombie?
    pub is_zombie: bool,
    /// memory set(address space)
    pub memory_set: MemorySet,
    /// parent process
    pub parent: Option<Weak<ProcessControlBlock>>,
    /// children process
    pub children: Vec<Arc<ProcessControlBlock>>,
    /// exit code
    pub exit_code: i32,
    /// file descriptor table
    pub fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>,
    /// signal flags
    pub signals: SignalFlags,
    /// tasks(also known as threads)
    pub tasks: Vec<Option<Arc<TaskControlBlock>>>,
    /// task resource allocator
    pub task_res_allocator: RecycleAllocator,
    /// mutex list
    pub mutex_list: Vec<Option<Arc<dyn Mutex>>>,
    /// semaphore list
    pub semaphore_list: Vec<Option<Arc<Semaphore>>>,
    /// condvar list
    pub condvar_list: Vec<Option<Arc<Condvar>>>,

    /// Available resource vector  
    /// 每一个信号量不会被置为none，因为我们没有检查它的生命周期
    pub avaliable_resouce: Vec<usize>,
    /// Allocation resouce matrix
    pub allocation_resource: Vec<Vec<usize>>,
    /// Needed resource vector
    pub need_resource: Vec<Vec<usize>>,
    /// flag of deadlock detection
    pub deadlock_detect: bool,
    /// map for resource
    pub source_map: BTreeMap<String, usize>,
}

impl ProcessControlBlockInner {
    #[allow(unused)]
    /// get the address of app's page table
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    /// allocate a new file descriptor
    pub fn alloc_fd(&mut self) -> usize {
        if let Some(fd) = (0..self.fd_table.len()).find(|fd| self.fd_table[*fd].is_none()) {
            fd
        } else {
            self.fd_table.push(None);
            self.fd_table.len() - 1
        }
    }
    /// allocate a new task id
    pub fn alloc_tid(&mut self) -> usize {
        self.task_res_allocator.alloc()
    }
    /// deallocate a task id
    pub fn dealloc_tid(&mut self, tid: usize) {
        self.task_res_allocator.dealloc(tid)
    }
    /// the count of tasks(threads) in this process
    pub fn thread_count(&self) -> usize {
        self.tasks.len()
    }
    /// get a task with tid in this process
    pub fn get_task(&self, tid: usize) -> Arc<TaskControlBlock> {
        self.tasks[tid].as_ref().unwrap().clone()
    }
    /// 新增线程时添加的资源控制块
    /// 注意线程没有父子继承关系，所以不会需要继承
    pub fn add_new_task_resouce(&mut self, tid: usize) {
        if tid >= self.allocation_resource.len() {
            self.allocation_resource.push(Vec::new());
            self.need_resource.push(Vec::new());
        }
        // 新增的线程应当是对所有资源量均没有占用
        // 线程的值即是在alloc的下标，因为分配tid是按照顺序来的
        self.allocation_resource[tid] = [0].repeat(self.avaliable_resouce.len());
        self.need_resource[tid] = [0].repeat(self.avaliable_resouce.len());
    }
    /// 为效率起见，本该直接返回符合条件的线程的id，但是懒了，交给调度器
    /// num 为线程请求的条件量
    /// rid 为资源条件量在自己的类型中的下标编号
    pub fn detect_for_resourece_request(&self) -> bool {
        // rid是全局资源量的下标
        let mut finish_status = [false].repeat(self.need_resource.len());
        let mut work = self.avaliable_resouce.clone();
        let mut finish_flag = true;
        // let mut task_id = 0;
        while finish_flag {
            finish_flag = false;
            for tid in 0..self.need_resource.len() {
                if finish_status[tid] {
                    continue;
                }
                let mut ok_status = true;
                // 判断进程可否结束
                for rid in 0..self.avaliable_resouce.len() {
                    if self.need_resource[tid][rid] > work[rid] {
                        ok_status = false;
                        break;
                    }
                }
                if ok_status {
                    for rid in 0..self.avaliable_resouce.len() {
                        work[rid] += self.allocation_resource[tid][rid];
                    }
                    finish_status[tid] = true;
                    finish_flag = true;
                }
            }
        }
        for tid in 0..self.need_resource.len() {
            if finish_status[tid] == false {
                return false;
                // 检测到死锁
            }
        }
        true
    }
    /// 为新资源分配一个编号
    pub fn alloc_index_for_resource(&mut self, index: usize, rtype: String, init_num: usize) {
        let key = rtype + index.to_string().as_str();
        assert!(self.source_map.get(&key).is_none());
        self.source_map.insert(key, self.avaliable_resouce.len());
        self.avaliable_resouce.push(init_num);
        for now_index in 0..self.allocation_resource.len() {
            self.allocation_resource[now_index].push(0);
            self.need_resource[now_index].push(0);
            // 认为没有线程需要新的资源，如果不是显示调用的话
        }
    }
    pub fn alloc_resource_for_task(&mut self, tid: usize, rid: usize) {
        // rid为全局资源下标编号
        assert!(self.need_resource[tid][rid] <= self.avaliable_resouce[rid]);
        self.avaliable_resouce[rid] -= self.need_resource[tid][rid];
        self.allocation_resource[tid][rid] += self.need_resource[tid][rid];
        self.need_resource[tid][rid] = 0;
        // 修改当前全局空闲资源量
    }

    /// 注意资源是逐个释放的，不要一次性全部放完
    /// exit_flag 代表线程是否完全退出
    pub fn dealloc_resource_for_task(&mut self, tid: usize, rid: usize, exit_flag: bool) {
        // rid为全局资源下标编号
        if exit_flag {
            // 释放全部资源
            for rindex in 0..self.avaliable_resouce.len() {
                self.avaliable_resouce[rindex] += self.allocation_resource[tid][rid];
                self.need_resource[tid][rindex] = 0;
                self.allocation_resource[tid][rindex] = 0;
            }
        } else {
            // 考虑到信号量的存在，所以我们需要单独特判
            self.avaliable_resouce[rid] += 1;
            if self.allocation_resource[tid][rid] != 0 {
                self.allocation_resource[tid][rid] -= 1;
            }
            // self.allocation_resource[tid][rid] = 0;
            self.need_resource[tid][rid] = 0;
        }
        // 修改当前全局空闲资源量
    }

    /// modify the need
    pub fn add_need(&mut self, tid: usize, rid: usize) {
        self.need_resource[tid][rid] += 1;
    }

    /// Get the index in avalibale resource for different type of resource
    pub fn get_index_for_resource(&self, index: usize, rtype: String) -> usize {
        let key = rtype + index.to_string().as_str();
        *self.source_map.get(&key).unwrap()
    }
}

impl ProcessControlBlock {
    /// inner_exclusive_access
    pub fn inner_exclusive_access(&self) -> RefMut<'_, ProcessControlBlockInner> {
        self.inner.exclusive_access()
    }
    /// new process from elf file
    pub fn new(elf_data: &[u8]) -> Arc<Self> {
        trace!("kernel: ProcessControlBlock::new");
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, ustack_base, entry_point) = MemorySet::from_elf(elf_data);
        // allocate a pid
        let pid_handle = pid_alloc();
        let process = Arc::new(Self {
            pid: pid_handle,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    memory_set,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: vec![
                        // 0 -> stdin
                        Some(Arc::new(Stdin)),
                        // 1 -> stdout
                        Some(Arc::new(Stdout)),
                        // 2 -> stderr
                        Some(Arc::new(Stdout)),
                    ],
                    signals: SignalFlags::empty(),
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    mutex_list: Vec::new(),
                    semaphore_list: Vec::new(),
                    condvar_list: Vec::new(),
                    avaliable_resouce: Vec::new(),
                    allocation_resource: Vec::new(),
                    need_resource: Vec::new(),
                    deadlock_detect: false,
                    source_map: BTreeMap::new(),
                })
            },
        });
        // create a main thread, we should allocate ustack and trap_cx here
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&process),
            ustack_base,
            true,
        ));
        // prepare trap_cx of main thread
        let task_inner = task.inner_exclusive_access();
        let task_id = task_inner.res.as_ref().unwrap().tid;
        let trap_cx = task_inner.get_trap_cx();
        let ustack_top = task_inner.res.as_ref().unwrap().ustack_top();
        let kstack_top = task.kstack.get_top();
        drop(task_inner);
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            ustack_top,
            KERNEL_SPACE.exclusive_access().token(),
            kstack_top,
            trap_handler as usize,
        );
        // add main thread to the process
        let mut process_inner = process.inner_exclusive_access();
        process_inner.tasks.push(Some(Arc::clone(&task)));
        process_inner.add_new_task_resouce(task_id);
        // 为初始进程分配资源
        drop(process_inner);
        insert_into_pid2process(process.getpid(), Arc::clone(&process));
        // add main thread to scheduler
        add_task(task);
        process
    }

    /// Only support processes with a single thread.
    pub fn exec(self: &Arc<Self>, elf_data: &[u8], args: Vec<String>) {
        trace!("kernel: exec");
        assert_eq!(self.inner_exclusive_access().thread_count(), 1);
        // memory_set with elf program headers/trampoline/trap context/user stack
        trace!("kernel: exec .. MemorySet::from_elf");
        let (memory_set, ustack_base, entry_point) = MemorySet::from_elf(elf_data);
        let new_token = memory_set.token();
        // substitute memory_set
        trace!("kernel: exec .. substitute memory_set");
        self.inner_exclusive_access().memory_set = memory_set;
        // then we alloc user resource for main thread again
        // since memory_set has been changed
        trace!("kernel: exec .. alloc user resource for main thread again");
        let task = self.inner_exclusive_access().get_task(0);
        let mut task_inner = task.inner_exclusive_access();
        task_inner.res.as_mut().unwrap().ustack_base = ustack_base;
        task_inner.res.as_mut().unwrap().alloc_user_res();
        task_inner.trap_cx_ppn = task_inner.res.as_mut().unwrap().trap_cx_ppn();
        // push arguments on user stack
        trace!("kernel: exec .. push arguments on user stack");
        let mut user_sp = task_inner.res.as_mut().unwrap().ustack_top();
        user_sp -= (args.len() + 1) * core::mem::size_of::<usize>();
        let argv_base = user_sp;
        let mut argv: Vec<_> = (0..=args.len())
            .map(|arg| {
                translated_refmut(
                    new_token,
                    (argv_base + arg * core::mem::size_of::<usize>()) as *mut usize,
                )
            })
            .collect();
        *argv[args.len()] = 0;
        for i in 0..args.len() {
            user_sp -= args[i].len() + 1;
            *argv[i] = user_sp;
            let mut p = user_sp;
            for c in args[i].as_bytes() {
                *translated_refmut(new_token, p as *mut u8) = *c;
                p += 1;
            }
            *translated_refmut(new_token, p as *mut u8) = 0;
        }
        // make the user_sp aligned to 8B for k210 platform
        user_sp -= user_sp % core::mem::size_of::<usize>();
        // initialize trap_cx
        trace!("kernel: exec .. initialize trap_cx");
        let mut trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            task.kstack.get_top(),
            trap_handler as usize,
        );
        trap_cx.x[10] = args.len();
        trap_cx.x[11] = argv_base;
        *task_inner.get_trap_cx() = trap_cx;
    }

    /// Only support processes with a single thread.
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        trace!("kernel: fork");
        let mut parent = self.inner_exclusive_access();
        assert_eq!(parent.thread_count(), 1);
        // clone parent's memory_set completely including trampoline/ustacks/trap_cxs
        let memory_set = MemorySet::from_existed_user(&parent.memory_set);
        // alloc a pid
        let pid = pid_alloc();
        // copy fd table
        let mut new_fd_table: Vec<Option<Arc<dyn File + Send + Sync>>> = Vec::new();
        for fd in parent.fd_table.iter() {
            if let Some(file) = fd {
                new_fd_table.push(Some(file.clone()));
            } else {
                new_fd_table.push(None);
            }
        }
        // create child process pcb
        let child = Arc::new(Self {
            pid,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    memory_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: new_fd_table,
                    signals: SignalFlags::empty(),
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    mutex_list: Vec::new(),
                    semaphore_list: Vec::new(),
                    condvar_list: Vec::new(),
                    avaliable_resouce: parent.avaliable_resouce.clone(),
                    allocation_resource: parent.allocation_resource.clone(),
                    need_resource: parent.need_resource.clone(),
                    deadlock_detect: false, // 子进程默认关闭死锁检测
                    source_map: parent.source_map.clone(),
                })
            },
        });
        // add child
        parent.children.push(Arc::clone(&child));
        // create main thread of child process
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&child),
            parent
                .get_task(0)
                .inner_exclusive_access()
                .res
                .as_ref()
                .unwrap()
                .ustack_base(),
            // here we do not allocate trap_cx or ustack again
            // but mention that we allocate a new kstack here
            false,
        ));
        // attach task to child process
        let mut child_inner = child.inner_exclusive_access();
        child_inner.tasks.push(Some(Arc::clone(&task)));
        drop(child_inner);
        // modify kstack_top in trap_cx of this thread
        let task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        trap_cx.kernel_sp = task.kstack.get_top();
        drop(task_inner);
        insert_into_pid2process(child.getpid(), Arc::clone(&child));
        // add this thread to scheduler
        add_task(task);
        child
    }
    /// get pid
    pub fn getpid(&self) -> usize {
        self.pid.0
    }
}

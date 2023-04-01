//! File and filesystem-related syscalls
use easy_fs::DiskInodeType;

use crate::fs::inode::{calc_nlink, find_id_by_name, linkat, unlinkat};
use crate::fs::{open_file, OpenFlags, Stat, StatMode};
use crate::mm::{translated_byte_buffer, translated_refmut, translated_str, UserBuffer};
use crate::task::{current_task, current_user_token};

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_write", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_read", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        trace!("kernel: sys_read .. file.read");
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_open(path: *const u8, flags: u32) -> isize {
    trace!("kernel:pid[{}] sys_open", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(inode) = open_file(path.as_str(), OpenFlags::from_bits(flags).unwrap()) {
        let mut inner = task.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    trace!("kernel:pid[{}] sys_close", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    0
}

/// YOUR JOB: Implement fstat.
pub fn sys_fstat(_fd: usize, _st: *mut Stat) -> isize {
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if inner.fd_table[_fd].is_none() {
        return -1;
    }
    let os_inode = inner.fd_table[_fd].as_ref().unwrap();
    let name = os_inode.name().clone();
    drop(os_inode);
    drop(inner);
    let inode = find_id_by_name(name.as_str());
    if inode.is_none() {
        return -1;
    }
    let inode = inode.unwrap();
    let inode_id = inode.inode_id;
    let nlink = calc_nlink(inode_id);
    let mode = match inode.inode_type() {
        DiskInodeType::File => StatMode::FILE,
        DiskInodeType::Directory => StatMode::DIR,
    };
    drop(inode);
    *translated_refmut(current_user_token(), _st) = Stat {
        dev: 0,
        ino: inode_id as u64,
        mode,
        nlink,
        pad: [0 as u64; 7],
    };
    return 0;
}

/// YOUR JOB: Implement linkat.
pub fn sys_linkat(_old_name: *const u8, _new_name: *const u8) -> isize {
    let token = current_user_token();
    let old_path = translated_str(token, _old_name);
    let new_path = translated_str(token, _new_name);
    if old_path == new_path {
        return -1;
    }
    linkat(old_path.as_str(), new_path.as_str())
}

/// YOUR JOB: Implement unlinkat.
pub fn sys_unlinkat(_name: *const u8) -> isize {
    let token = current_user_token();
    let path = translated_str(token, _name);
    unlinkat(path.as_str())
}

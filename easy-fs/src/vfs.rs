use super::{
    block_cache_sync_all, get_block_cache, BlockDevice, DirEntry, DiskInode, DiskInodeType,
    EasyFileSystem, DIRENT_SZ,
};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};
/// Virtual filesystem layer over easy-fs
pub struct Inode {
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
    /// the id of the inode
    pub inode_id: u32,
}

impl Inode {
    /// Create a vfs inode
    pub fn new(
        block_id: u32,
        block_offset: usize,
        fs: Arc<Mutex<EasyFileSystem>>,
        block_device: Arc<dyn BlockDevice>,
        inode_id: u32,
    ) -> Self {
        Self {
            block_id: block_id as usize,
            block_offset,
            fs,
            block_device,
            inode_id,
        }
    }
    /// Call a function over a disk inode to read it
    fn read_disk_inode<V>(&self, f: impl FnOnce(&DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .read(self.block_offset, f)
    }
    /// Call a function over a disk inode to modify it
    fn modify_disk_inode<V>(&self, f: impl FnOnce(&mut DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .modify(self.block_offset, f)
    }
    /// Find inode under a disk inode by name
    fn find_inode_id(&self, name: &str, disk_inode: &DiskInode) -> Option<u32> {
        // assert it is a directory
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut dirent = DirEntry::empty();
        for i in 0..file_count {
            assert_eq!(
                disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                DIRENT_SZ,
            );
            if dirent.name() == name {
                return Some(dirent.inode_id() as u32);
            }
        }
        None
    }
    /// return the type of the inode
    pub fn inode_type(&self) -> DiskInodeType {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.type_.clone())
    }
    /// Find inode under current inode by name
    pub fn find(&self, name: &str) -> Option<Arc<Inode>> {
        let fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            self.find_inode_id(name, disk_inode).map(|inode_id| {
                let (block_id, block_offset) = fs.get_disk_inode_pos(inode_id);
                Arc::new(Self::new(
                    block_id,
                    block_offset,
                    self.fs.clone(),
                    self.block_device.clone(),
                    inode_id,
                ))
            })
        })
    }

    /// Increase the size of a disk inode
    fn increase_size(
        &self,
        new_size: u32,
        disk_inode: &mut DiskInode,
        fs: &mut MutexGuard<EasyFileSystem>,
    ) {
        if new_size < disk_inode.size {
            return;
        }
        let blocks_needed = disk_inode.blocks_num_needed(new_size);
        let mut v: Vec<u32> = Vec::new();
        for _ in 0..blocks_needed {
            v.push(fs.alloc_data());
        }
        disk_inode.increase_size(new_size, v, &self.block_device);
    }
    /// Create inode under current inode by name
    pub fn create(&self, name: &str) -> Option<Arc<Inode>> {
        let mut fs = self.fs.lock();
        let op = |root_inode: &DiskInode| {
            // assert it is a directory
            assert!(root_inode.is_dir());
            // has the file been created?
            self.find_inode_id(name, root_inode)
        };
        if self.read_disk_inode(op).is_some() {
            return None;
        }
        // create a new file
        // alloc a inode with an indirect block
        let new_inode_id = fs.alloc_inode();
        // initialize inode
        let (new_inode_block_id, new_inode_block_offset) = fs.get_disk_inode_pos(new_inode_id);
        get_block_cache(new_inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(new_inode_block_offset, |new_inode: &mut DiskInode| {
                new_inode.initialize(DiskInodeType::File);
            });
        self.modify_disk_inode(|root_inode| {
            // append file in the dirent
            let file_count = (root_inode.size as usize) / DIRENT_SZ;
            let new_size = (file_count + 1) * DIRENT_SZ;
            // increase size
            self.increase_size(new_size as u32, root_inode, &mut fs);
            // write dirent
            let dirent = DirEntry::new(name, new_inode_id);
            root_inode.write_at(
                file_count * DIRENT_SZ,
                dirent.as_bytes(),
                &self.block_device,
            );
        });

        let (block_id, block_offset) = fs.get_disk_inode_pos(new_inode_id);
        block_cache_sync_all();
        // return inode
        Some(Arc::new(Self::new(
            block_id,
            block_offset,
            self.fs.clone(),
            self.block_device.clone(),
            new_inode_id,
        )))
        // release efs lock automatically by compiler
    }
    /// create a link to a existing inode
    pub fn linkat(&self, old_path: &str, new_path: &str) -> isize {
        // 注意：不需要重新分配块，只需要添加目录项即可
        // new_path对应的新的目录项的inode_id是old_path对应的inode的id
        let mut fs = self.fs.lock();
        let val = self.modify_disk_inode(|root_inode| {
            if let Some(old_id) = self.find_inode_id(old_path, &root_inode) {
                let new_dirent = DirEntry::new(new_path, old_id);
                let file_count = (root_inode.size as usize) / DIRENT_SZ;
                let new_size = (file_count + 1) * DIRENT_SZ;
                // increase size
                self.increase_size(new_size as u32, root_inode, &mut fs);
                root_inode.write_at(
                    file_count * DIRENT_SZ,
                    new_dirent.as_bytes(),
                    &self.block_device,
                );
                return 0;
            } else {
                // 旧文件不存在
                return -1;
            }
        });
        block_cache_sync_all();
        val
    }
    /// unlink the link to a inode
    pub fn unlinkat(&self, path: &str) -> isize {
        // 不考虑目录项的内存的回收，开摆！
        let mut fs = self.fs.lock();
        // 直接把目录项的内容清空即可
        let val = self.modify_disk_inode(|root_inode| {
            let file_count = (root_inode.size as usize) / DIRENT_SZ;
            let mut dirent = DirEntry::empty();
            let mut dirent_id: isize = -1;
            for i in 0..file_count {
                assert_eq!(
                    root_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                if dirent.name() == path {
                    dirent_id = i as isize;
                    break;
                }
            }
            if dirent_id == -1 {
                // 文件不存在
                return -1;
            }
            let inode_id = dirent.inode_id();
            root_inode.write_at(
                dirent_id as usize * DIRENT_SZ,
                [0 as u8; DIRENT_SZ].as_slice(),
                &self.block_device,
            );
            // 检查当前的inode_id是否已经没有链接项了
            // if self.find_inode_id(path, &root_inode).is_none() {
            //     // 释放inode对应的内存
            //     let (block_id, block_offset) = fs.get_disk_inode_pos(inode_id);
            //     let new_inode = Self::new(
            //         block_id,
            //         block_offset,
            //         self.fs.clone(),
            //         self.block_device.clone(),
            //         inode_id,
            //     );
            //     new_inode.clear();
            // }
            for i in 0..file_count {
                assert_eq!(
                    root_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                if dirent.inode_id() == inode_id && dirent.name() != "" {
                    return 0;
                }
            }
            // 若可以运行到这里，说明没有链接项
            let (block_id, block_offset) = fs.get_disk_inode_pos(inode_id);
            let new_inode = Self::new(
                block_id,
                block_offset,
                self.fs.clone(),
                self.block_device.clone(),
                inode_id,
            );
            // 不可以直接调用clear函数，重复申请fs导致死锁
            new_inode.modify_disk_inode(|disk_inode| {
                let size = disk_inode.size;
                let data_blocks_dealloc = disk_inode.clear_size(&new_inode.block_device);
                assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
                for data_block in data_blocks_dealloc.into_iter() {
                    fs.dealloc_data(data_block);
                }
            });
            return 0;
        });

        block_cache_sync_all();
        val
    }
    /// calc the nlink of the inode
    pub fn calc_nlink(&self, inode_id: u32) -> u32 {
        let _fs = self.fs.lock();
        // 暴力出奇迹
        self.modify_disk_inode(|root_inode| {
            let mut nlink = 0;
            let file_count = (root_inode.size as usize) / DIRENT_SZ;
            let mut dirent = DirEntry::empty();
            for i in 0..file_count {
                assert_eq!(
                    root_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                if dirent.inode_id() == inode_id as u32 {
                    nlink += 1;
                }
            }
            nlink
        })
    }

    /// List inodes under current inode
    pub fn ls(&self) -> Vec<String> {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            let mut v: Vec<String> = Vec::new();
            for i in 0..file_count {
                let mut dirent = DirEntry::empty();
                assert_eq!(
                    disk_inode.read_at(i * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                v.push(String::from(dirent.name()));
            }
            v
        })
    }
    /// Read data from current inode
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.read_at(offset, buf, &self.block_device))
    }
    /// Write data to current inode
    pub fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut fs = self.fs.lock();
        let size = self.modify_disk_inode(|disk_inode| {
            self.increase_size((offset + buf.len()) as u32, disk_inode, &mut fs);
            disk_inode.write_at(offset, buf, &self.block_device)
        });
        block_cache_sync_all();
        size
    }
    /// Clear the data in current inode
    pub fn clear(&self) {
        let mut fs = self.fs.lock();
        self.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;

            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });
        block_cache_sync_all();
    }
}

use std::{
    collections::HashMap,
    io::{self, Error, ErrorKind},
    sync::Arc,
};

use tokio::sync::RwLock;

use crate::{
    block::{
        clear_blocks, get_block_mut, get_blocks_buffers, read_blocks_to_cache, Block,
        BLOCK_CACHE_MANAGER,
    },
    fs_constants::*,
};

type BitmapDataType = bitmaps::Bitmap<8>;

#[derive(Default)]
pub struct BitmapManager {
    inodes: Vec<BitmapDataType>, // 以字节为单位存储inode位图缓存
    datas: Vec<BitmapDataType>,  // 以字节为单位存储data位图缓存
    last_inode_byte_pos: usize,  // 最后一次alloc inode bit所在的byte的位置
    last_data_byte_pos: usize,   // 最后一次alloc data bit所在的byte的位置
}

impl BitmapManager {
    pub async fn read(&mut self) -> io::Result<()> {
        // 读入位图区快
        let range = INODE_BITMAP_START_BLOCK..DATA_BITMAP_START_BLOCK + DATA_BITMAP_NUM;
        let mut block_args = Vec::new();
        for block_id in range {
            block_args.push((block_id, 0, BLOCK_SIZE));
        }
        let buffers = get_blocks_buffers(&block_args).await?;

        // 读入inode位图
        let mut inodes = Vec::new();
        for block_buffer in &buffers[..INODE_BITMAP_NUM] {
            for byte in block_buffer {
                let bitmap: BitmapDataType = bitmaps::Bitmap::from_value(*byte);
                inodes.push(bitmap);
            }
        }

        // 读入data位图
        let mut datas = Vec::new();
        for block_buffer in &buffers[INODE_BITMAP_NUM..] {
            for byte in block_buffer {
                let bitmap: BitmapDataType = bitmaps::Bitmap::from_value(*byte);
                datas.push(bitmap);
            }
        }

        *self = Self {
            inodes,
            datas,
            last_inode_byte_pos: 0,
            last_data_byte_pos: 0,
        };

        Ok(())
    }

    /// 返回bit_id
    fn alloc_bit(&mut self, bitmap_type: BitmapType) -> io::Result<u32> {
        let (bitmap, prev_byte_pos) = match bitmap_type {
            BitmapType::Inode => (&mut self.inodes, &mut self.last_inode_byte_pos),
            BitmapType::Data => (&mut self.datas, &mut self.last_data_byte_pos),
        };

        let mut cur_byte_pos = *prev_byte_pos;
        loop {
            let byte = &mut bitmap[cur_byte_pos];
            // 如果找到了非全满的byte
            if let Some(bit_pos) = byte.first_false_index() {
                let id = cur_byte_pos * 8 + bit_pos;
                byte.set(bit_pos, true); // 设置为已占用
                *prev_byte_pos = cur_byte_pos; // 更新位置
                return Ok(id as u32);
            }

            cur_byte_pos = (cur_byte_pos + 1) % bitmap.len();
            if cur_byte_pos == *prev_byte_pos {
                // 回到了同一个位置还没找到
                break;
            }
        }
        Err(Error::new(ErrorKind::OutOfMemory, "no valid bit"))
    }

    // 返回false如果bit本身已经是0
    fn dealloc_bit(&mut self, bitmap_type: BitmapType, bit_id: usize) -> bool {
        let bitmap = match bitmap_type {
            BitmapType::Inode => &mut self.inodes,
            BitmapType::Data => &mut self.datas,
        };

        let byte_pos = bit_id / 8;
        let bit_pos = bit_id % 8;
        // 因为set返回的是之前的bit值而不是有没有设置成功，所以返回true代表原来是1，可以dealloc
        bitmap[byte_pos].set(bit_pos, false)
    }

    /// 读入所有位图区块缓存
    pub async fn cache_to_block(&self) -> io::Result<()> {
        let block_ids: Vec<_> =
            (INODE_BITMAP_START_BLOCK..DATA_BITMAP_START_BLOCK + DATA_BITMAP_NUM).collect();
        read_blocks_to_cache(&block_ids).await?;

        // 取锁
        let blk = Arc::clone(&BLOCK_CACHE_MANAGER);
        let mut bcm = blk.write().await;
        let block_cache = &mut bcm.block_cache;

        // 写入inode位图块
        let inode_block_ids = &block_ids[..INODE_BITMAP_NUM];
        self.write_bitmaps_to_blocks(inode_block_ids, BitmapType::Inode, block_cache)?;

        // 写入data位图块
        let data_block_ids = &block_ids[INODE_BITMAP_NUM..];
        self.write_bitmaps_to_blocks(data_block_ids, BitmapType::Data, block_cache)
    }

    /// 将位图缓存写入块缓存
    fn write_bitmaps_to_blocks(
        &self,
        block_ids: &[usize],
        bitmap_type: BitmapType,
        block_cache: &mut HashMap<usize, Block>,
    ) -> io::Result<()> {
        let (start_block_id, bitmap) = match bitmap_type {
            BitmapType::Inode => (INODE_BITMAP_START_BLOCK, &self.inodes),
            BitmapType::Data => (DATA_BITMAP_START_BLOCK, &self.datas),
        };
        for block_id in block_ids {
            let inner_block_index = block_id - start_block_id; // 计算这个块是对应位图块中的第几块
            let cache_start_index = inner_block_index * BLOCK_SIZE; // 计算在缓存vec中的起始偏移量
            let chche_end_index = (inner_block_index + 1) * BLOCK_SIZE; // 计算在缓存vec中的终止偏移量

            let buffers: Vec<_> = bitmap[cache_start_index..chche_end_index] // 将位图缓存中的bitmap转换成u8
                .iter()
                .map(|bitmap| bitmap.into_value())
                .collect();

            let block = get_block_mut(block_id, block_ids, block_cache)?;
            block.modify_bytes(|bytes| bytes.clone_from_slice(&buffers));
        }
        Ok(())
    }
}

/// 获取一个空闲bit的位置，如果有，则bit置1并返回位置
/// 这个位置是从当前所属位图开始计算，即当前所属位图的第K个bit
pub async fn alloc_bit(bitmap_type: BitmapType) -> Result<u32, Error> {
    let bit_id = Arc::clone(&BITMAP_MANAGER)
        .write()
        .await
        .alloc_bit(bitmap_type)?;

    trace!("alloc id {} for a {:?}", bit_id, bitmap_type);
    Ok(bit_id)
}

/// 在inode位图中dealloc对应的bit
pub async fn dealloc_inode_bit(inode_id: usize) -> bool {
    Arc::clone(&BITMAP_MANAGER)
        .write()
        .await
        .dealloc_bit(BitmapType::Inode, inode_id)
}

/// 在对应的位图中dealloc 指定block所占用的bit, 同时清空该block
pub async fn dealloc_data_bit(block_id: usize) {
    let id = [block_id];
    dealloc_data_bits(&id).await;
}

/// 批量清除data block并dealloc
pub async fn dealloc_data_bits(block_ids: &[usize]) {
    // 取得bitmap manager的可变引用
    let bitmap_manager = Arc::clone(&BITMAP_MANAGER);
    let mut bitmap_write_lock = bitmap_manager.write().await;

    let mut block_to_clear = Vec::new();
    for block_id in block_ids {
        // 在位图缓存中试图dealloc这个block
        let bit_id = block_id - DATA_START_BLOCK;
        let success = bitmap_write_lock.dealloc_bit(BitmapType::Data, bit_id);
        if success {
            // 准备清空该磁盘块内容
            block_to_clear.push(*block_id);
        }
    }
    clear_blocks(&block_to_clear).await.unwrap();
}

async fn count_bits(bitmap_type: BitmapType) -> usize {
    //读取缓存
    let bitmap_manager = Arc::clone(&BITMAP_MANAGER);
    let read_lock = bitmap_manager.read().await;
    let bitmap = match bitmap_type {
        BitmapType::Inode => &read_lock.inodes,
        BitmapType::Data => &read_lock.datas,
    };
    bitmap.iter().map(|byte| byte.len()).sum()
}

async fn get_bitmaps(bitmap_type: BitmapType) -> Vec<BitmapDataType> {
    //读取缓存
    let bitmap_manager = Arc::clone(&BITMAP_MANAGER);
    let read_lock = bitmap_manager.read().await;
    let bitmap = match bitmap_type {
        BitmapType::Inode => &read_lock.inodes,
        BitmapType::Data => &read_lock.datas,
    };
    bitmap.clone()
}

pub async fn get_inode_bitmaps() -> Vec<BitmapDataType> {
    get_bitmaps(BitmapType::Inode).await
}

pub async fn get_data_bitmaps() -> Vec<BitmapDataType> {
    get_bitmaps(BitmapType::Data).await
}

/// 统计申请了多少inode,第一个返回值为已申请，第二个返回值为未申请
pub async fn count_inodes() -> (usize, usize) {
    let alloced = count_bits(BitmapType::Inode).await;
    (alloced, INODE_MAX_NUM - alloced)
}

/// 统计申请了多少数据块,第一个返回值为已申请，第二个返回值为未申请
pub async fn count_data_blocks() -> (usize, usize) {
    let alloced = count_bits(BitmapType::Data).await;
    (alloced, DATA_BLOCK_MAX_NUM - alloced)
}

/// 统计空闲data block数
pub async fn count_valid_data_blocks() -> usize {
    DATA_BLOCK_MAX_NUM - count_bits(BitmapType::Data).await
}

#[derive(Debug, Clone, Copy)]
pub enum BitmapType {
    Inode,
    Data,
}

//延迟加载全局变量 BITMAP_MANAGER
lazy_static! {
    pub static ref BITMAP_MANAGER: Arc<RwLock<BitmapManager>> =
        Arc::new(RwLock::new(BitmapManager::default()));
}

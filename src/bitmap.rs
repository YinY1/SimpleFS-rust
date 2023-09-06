use log::trace;

use crate::{
    block::{get_block_buffer, read_block_to_cache, BLOCK_CACHE_MANAGER},
    simple_fs::*,
};

/// 获取一个空闲bit的位置，如果有，则bit置1并返回位置(即返回一个id，inode id或块id)
pub fn alloc_bit(bitmap_type: BitmapType) -> Option<u16> {
    let (block_nums, block_start_id) = match bitmap_type {
        BitmapType::Inode => (INODE_BITMAP_NUM, INODE_BITMAP_BLOCK),
        BitmapType::Data => (DATA_BITMAP_NUM, DATA_BITMAP_BLOCK),
    };

    for i in 0..block_nums {
        let block_id = block_start_id + i;
        read_block_to_cache(block_id);

        let mut bcm = BLOCK_CACHE_MANAGER.lock();

        for block in &mut bcm.block_cache {
            if block.block_id == block_id {
                for (i, byte) in block.bytes.iter_mut().enumerate() {
                    if *byte == 0b11111111 {
                        continue;
                    }
                    // 从高位到低位遍历（从左到右）
                    for j in (0..8).rev() {
                        let bit = (*byte >> j) & 1;
                        if bit == 0 {
                            // 找到空闲bit
                            *byte |= 1 << j;
                            block.modified = true;
                            let id = (i * 8 + 7 - j) as u16;
                            trace!("alloc id {} for a {:?}", id, bitmap_type);
                            return Some(id);
                        }
                    }
                }
            }
        }
    }
    // 没有空余位图了
    None
}

pub fn dealloc_bit(bitmap_type: BitmapType, id: usize) -> bool {
    let block_start_id = match bitmap_type {
        BitmapType::Inode => INODE_BITMAP_BLOCK,
        BitmapType::Data => DATA_BITMAP_BLOCK,
    };

    let byte_pos = id / 8;
    let bit_pos = id % 8;
    let block_id = byte_pos / BLOCK_SIZE + block_start_id;

    read_block_to_cache(block_id);
    let mut bcm = BLOCK_CACHE_MANAGER.lock();

    for block in &mut bcm.block_cache {
        if block.block_id == block_id {
            let byte = &mut block.bytes[byte_pos];
            if *byte & 1 << bit_pos == 1 {
                *byte &= !(1 << bit_pos);
                return true;
            } else {
                //该位bit没有占用 不需要dealloc
                return false;
            }
        }
    }
    false
}

fn count_bits(bitmap_type: BitmapType) -> usize {
    let (start_id, block_nums) = match bitmap_type {
        BitmapType::Inode => (INODE_BITMAP_BLOCK, INODE_BITMAP_NUM),
        BitmapType::Data => (DATA_BITMAP_BLOCK, DATA_BITMAP_NUM),
    };
    (0..block_nums)
        .map(|i| {
            let block_id = start_id + i;
            let bm = get_block_buffer(block_id, 0, BLOCK_SIZE).unwrap();
            bm.iter()
                .map(|byte| byte.count_ones() as usize)
                .sum::<usize>()
        })
        .sum()
}

/// 统计申请了多少inode
pub fn count_inodes() -> usize {
    count_bits(BitmapType::Inode)
}

/// 统计申请了多少数据块
pub fn count_data_blocks() -> usize {
    count_bits(BitmapType::Data)
}

#[derive(Debug)]
pub enum BitmapType {
    Inode,
    Data,
}

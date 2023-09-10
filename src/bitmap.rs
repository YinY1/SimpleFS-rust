use log::{info, trace};

use crate::{
    block::{get_block_buffer, read_block_to_cache, BLOCK_CACHE_MANAGER},
    simple_fs::*,
};

/// 获取一个空闲bit的位置，如果有，则bit置1并返回位置
/// 这个位置是从当前所属位图开始计算，即当前所属位图的第K个bit
pub fn alloc_bit(bitmap_type: BitmapType) -> Option<u32> {
    let (block_nums, block_start_id) = match bitmap_type {
        BitmapType::Inode => (INODE_BITMAP_NUM, INODE_BITMAP_BLOCK),
        BitmapType::Data => (DATA_BITMAP_NUM, DATA_BITMAP_BLOCK),
    };

    // 遍历位图的每个块
    for n in 0..block_nums {
        // 计算当前所在的块的id（起始id是super的0）
        let block_id = block_start_id + n;
        read_block_to_cache(block_id);

        let mut bcm = BLOCK_CACHE_MANAGER.lock();

        for block in &mut bcm.block_cache {
            if block.block_id == block_id {
                // 遍历当前块的每个byte, i [0,BLOCK_SIZE)
                for (i, byte) in block.bytes.iter_mut().enumerate() {
                    if *byte == 0b11111111 {
                        continue;
                    }
                    // 从高位到低位遍历当前byte的每个bit（从左到右）
                    for j in (0..8).rev() {
                        let bit = (*byte >> j) & 1;
                        if bit == 0 {
                            // 找到空闲bit
                            let id = (n * BLOCK_SIZE + i * 8 + 7 - j) as u32;
                            if let BitmapType::Data = bitmap_type {
                                if id as usize >= DATA_NUM {
                                    // 块id虽然能在位图中表示，但是超出了数据区块的数目
                                    info!("block id {} out of limit {}", id, DATA_NUM);
                                    return None;
                                }
                            }
                            block.modify_bytes(|bytes_arr| bytes_arr[i] |= 1 << j);
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

pub fn dealloc_inode_bit(inode_id: usize) -> bool {
    dealloc_bit(INODE_BITMAP_BLOCK, inode_id / 8, inode_id)
}

/// 在对应的位图中dealloc 指定block所占用的bit
pub fn dealloc_data_bit(block_id: usize) -> bool {
    let (bit_block_start_id, block_start_id) = (DATA_BITMAP_BLOCK, DATA_BLOCK);
    //对应位图（包括所有的块）中的总共第K个bit（从左到右）
    let bit_id = block_id - block_start_id;
    //对应位图（包括所有的块）中的总共第K个byte（从左到右）
    let total_byte_pos = bit_id / 8;
    //单个byte中的第K个bit（从左到右）
    let bit_pos = bit_id % 8;
    //这个bit所在的块的块号（从超级块sp=0开始）
    let bitmap_block_id = total_byte_pos / BLOCK_SIZE + bit_block_start_id;
    //在单个块中的第K个byte（从左到右）
    let inner_byte_pos = total_byte_pos % BLOCK_SIZE;

    dealloc_bit(bitmap_block_id, inner_byte_pos, bit_pos)
}

fn dealloc_bit(bitmap_block_id: usize, inner_byte_pos: usize, bit_pos: usize) -> bool {
    //将含有该bit的位图区域的块读入缓存
    read_block_to_cache(bitmap_block_id);

    let mut bcm = BLOCK_CACHE_MANAGER.lock();
    for block in &mut bcm.block_cache {
        if block.block_id == bitmap_block_id {
            let byte = &mut block.bytes[inner_byte_pos];
            // 从左到右的掩码（而不是从右到左，因为pos是从左开始计算的）
            let mask = 0b10000000 >> bit_pos;
            if (*byte & mask) != 0 {
                block.modify_bytes(|bytes_arr| bytes_arr[inner_byte_pos] &= !mask);
                return true;
            } else {
                //该位bit没有占用 不需要dealloc
                return false;
            }
        }
    }
    panic!("unreachable");
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

#[allow(unused)]
/// 统计申请了多少inode,第一个返回值为已申请，第二个返回值为未申请
pub fn count_inodes() -> (usize, usize) {
    let alloced = count_bits(BitmapType::Inode);
    (alloced, INODE_NUM - alloced)
}

#[allow(unused)]
/// 统计申请了多少数据块,第一个返回值为已申请，第二个返回值为未申请
pub fn count_data_blocks() -> (usize, usize) {
    let alloced = count_bits(BitmapType::Data);
    (alloced, DATA_NUM - alloced)
}

#[allow(unused)]
/// 统计空闲inode数
pub fn count_valid_inodes() -> usize {
    INODE_NUM - count_bits(BitmapType::Inode)
}

#[allow(unused)]
/// 统计空闲data block数
pub fn count_valid_data_blocks() -> usize {
    DATA_NUM - count_bits(BitmapType::Data)
}

#[derive(Debug, Clone, Copy)]
pub enum BitmapType {
    Inode,
    Data,
}

//! Runtime walk state.
use crate::core::filesystem::tree::fs_tree::FsWalkEntry;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

/// 一条全显实时遍历缓存的用途:整个起点子树的文件行(索引与快照都不可用
/// 的回退路径),或索引范围内已删除目录子树的实时拼接行(索引从不下探
/// 已删目录)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlatWalkKind {
    Subtree,
    DeletedSplice,
}

/// 一次实时遍历物化出的文件行(路径相对起点)。`deleted_dirs` 记录拼接行
/// 对应的已删目录集合,索引侧报出的集合变化时缓存作废重走。
pub struct FlatWalkRows {
    pub entries: Vec<FsWalkEntry>,
    pub truncated: bool,
    pub deleted_dirs: Vec<(String, String)>,
}

/// 文件全显实时遍历结果缓存,键 `(image_id, parent_id, 用途)`。全显是分页
/// 查询,翻页 / 换排序 / 改过滤都会再次请求同一起点;有索引时是 SQL,没有
/// 索引时若每页重走文件系统就不可用,于是把物化出的文件行缓存起来供同一
/// 起点复用。一条就可能是整卷的文件行,FIFO 上限 2 条;前端「刷新」显式
/// 作废对应条目。
#[derive(Default)]
pub struct FlatWalkCache {
    map: HashMap<(String, String, FlatWalkKind), Arc<FlatWalkRows>>,
    order: VecDeque<(String, String, FlatWalkKind)>,
}

impl FlatWalkCache {
    const MAX_ENTRIES: usize = 2;

    pub fn get(
        &self,
        image_id: &str,
        parent_id: &str,
        kind: FlatWalkKind,
    ) -> Option<Arc<FlatWalkRows>> {
        self.map
            .get(&(image_id.to_string(), parent_id.to_string(), kind))
            .cloned()
    }

    pub fn put(
        &mut self,
        image_id: &str,
        parent_id: &str,
        kind: FlatWalkKind,
        rows: Arc<FlatWalkRows>,
    ) {
        let key = (image_id.to_string(), parent_id.to_string(), kind);
        if self.map.insert(key.clone(), rows).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > Self::MAX_ENTRIES {
            if let Some(evicted) = self.order.pop_front() {
                self.map.remove(&evicted);
            }
        }
    }

    /// 卸载镜像时丢弃它的全部缓存行(一条就可能是整卷文件行)。
    pub fn purge_image(&mut self, image_id: &str) {
        self.map
            .retain(|(cached_image, _, _), _| cached_image != image_id);
        self.order
            .retain(|(cached_image, _, _)| cached_image != image_id);
    }

    /// 作废一个起点的全部缓存(手动刷新)。
    pub fn invalidate(&mut self, image_id: &str, parent_id: &str) {
        let stale = |cached_image: &String, cached_parent: &String| {
            cached_image == image_id && cached_parent == parent_id
        };
        self.map
            .retain(|(cached_image, cached_parent, _), _| !stale(cached_image, cached_parent));
        self.order
            .retain(|(cached_image, cached_parent, _)| !stale(cached_image, cached_parent));
    }
}

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

use lamfold::{BlockSource, FoldError, NodeId, Result};

use crate::geometry::{FREE, Geometry, INDEX, PAGE_HEADER, Page};

const NAME: usize = 13;
const INDEX_TABLE: usize = 8;

pub(crate) struct File {
    pub name: String,
    pub size: u64,
    pub pages: std::result::Result<Vec<u16>, &'static str>,
}

pub(crate) struct Catalog {
    pub files: BTreeMap<NodeId, File>,
    pub names: HashMap<String, NodeId>,
}

struct Allocations {
    pages: Vec<Option<Page>>,
    indexes: HashMap<(u16, u16), Vec<u16>>,
}

impl Allocations {
    fn load(source: &mut impl BlockSource, layout: Geometry) -> Result<Self> {
        let mut pages = vec![None; layout.pages()];
        let mut indexes = HashMap::<_, Vec<u16>>::new();
        let mut lookups = vec![0; layout.lookup_bytes()];
        let mut header = [0; PAGE_HEADER];
        let mut missing_signature = false;
        for block in 0..layout.blocks {
            source.read_at((block * layout.block_bytes) as u64, &mut lookups)?;
            layout.verify_signature(block, &lookups, &mut missing_signature)?;
            for (entry, raw) in lookups[..layout.entries() * 2]
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
            {
                let id = layout.order.u16(raw);
                if id == FREE || id == 0 {
                    continue;
                }
                let index = block * layout.pages_per_block() + layout.lookup_pages() + entry;
                source.read_at((index * layout.page_bytes) as u64, &mut header)?;
                let page = Page::decode(&header, layout.order);
                if page.id != id || !page.live() {
                    continue;
                }
                pages[index] = Some(page);
                if page.is_index() {
                    indexes
                        .entry((id, page.span))
                        .or_default()
                        .push(index as u16);
                }
            }
        }
        Ok(Self { pages, indexes })
    }

    fn is_data(&self, index: u16, object: u16, span: usize) -> bool {
        self.pages.get(usize::from(index)).is_some_and(|page| {
            page.is_some_and(|page| page.id == object && usize::from(page.span) == span)
        })
    }

    fn data_pages(
        &self,
        source: &mut impl BlockSource,
        layout: Geometry,
        object: u16,
        root: &[u8],
        after_name: usize,
        needed: usize,
    ) -> Result<std::result::Result<Vec<u16>, &'static str>> {
        if needed == 0 {
            return Ok(Ok(Vec::new()));
        }
        let mut found = None;
        let mut continuation = vec![0; layout.page_bytes];
        let per_table = (layout.page_bytes - INDEX_TABLE) / 2;
        for table in after_name..root.len() - 1 {
            if !self.is_data(layout.order.u16(&root[table..]), object, 0) {
                continue;
            }
            let mut pages = Vec::with_capacity(needed);
            self.extend(&mut pages, layout, object, &root[table..], needed);
            if pages.len() < needed.min((root.len() - table) / 2) {
                continue;
            }

            for span in 1..=needed.saturating_sub(pages.len()).div_ceil(per_table) {
                let Some(indexes) = self.indexes.get(&(object | INDEX, span as u16)) else {
                    break;
                };
                if indexes.len() != 1 {
                    break;
                }
                source.read_at(
                    u64::from(indexes[0]) * layout.page_bytes as u64,
                    &mut continuation,
                )?;
                let expected = needed.min(pages.len() + per_table);
                self.extend(
                    &mut pages,
                    layout,
                    object,
                    &continuation[INDEX_TABLE..],
                    needed,
                );
                if pages.len() < expected {
                    break;
                }
            }
            if pages.len() == needed {
                if found.as_ref().is_some_and(|previous| previous != &pages) {
                    return Ok(Err("ambiguous SPIFFS index table geometry"));
                }
                found = Some(pages);
            }
        }
        Ok(found.ok_or("SPIFFS file index does not map every declared data span"))
    }

    fn extend(
        &self,
        pages: &mut Vec<u16>,
        layout: Geometry,
        object: u16,
        table: &[u8],
        needed: usize,
    ) {
        for raw in table.as_chunks::<2>().0.iter().take(needed - pages.len()) {
            let index = layout.order.u16(raw);
            if !self.is_data(index, object, pages.len()) {
                break;
            }
            pages.push(index);
        }
    }
}

impl Catalog {
    pub fn load(source: &mut impl BlockSource, layout: Geometry) -> Result<Self> {
        let allocations = Allocations::load(source, layout)?;
        let mut files = BTreeMap::new();
        let mut names = HashMap::new();
        let mut root = vec![0; layout.page_bytes];
        for (&(id, span), indexes) in &allocations.indexes {
            if span != 0 {
                continue;
            }
            if indexes.len() != 1 {
                return Err(FoldError::Corrupt(
                    "SPIFFS object has multiple live index headers",
                ));
            }
            let physical = indexes[0];
            source.read_at(u64::from(physical) * layout.page_bytes as u64, &mut root)?;
            if root[12] != 1 {
                return Err(FoldError::Unsupported("unknown SPIFFS object type"));
            }
            let Some(end) = root[NAME..].iter().position(|&value| value == 0) else {
                return Err(FoldError::Corrupt("unterminated SPIFFS object name"));
            };
            if end == 0 {
                return Err(FoldError::Corrupt("empty SPIFFS object name"));
            }
            let name = display_name(&root[NAME..NAME + end]);
            let size = match layout.order.u32(&root[8..]) {
                u32::MAX => 0,
                size => u64::from(size),
            };
            let needed = size.div_ceil(layout.data_bytes() as u64) as usize;
            let pages = if needed > layout.pages() {
                Err("SPIFFS file exceeds volume geometry")
            } else {
                allocations.data_pages(
                    source,
                    layout,
                    id & !INDEX,
                    &root,
                    NAME + end + 1,
                    needed,
                )?
            };
            let node = u64::from(physical);
            if names.insert(name.clone(), node).is_some() {
                return Err(FoldError::Corrupt("duplicate SPIFFS object name"));
            }
            files.insert(node, File { name, size, pages });
        }
        Ok(Self { files, names })
    }
}

fn display_name(raw: &[u8]) -> String {
    if raw == b"." || raw == b".." {
        return "%2E".repeat(raw.len());
    }
    let mut name = String::new();
    for part in raw.utf8_chunks() {
        for character in part.valid().chars() {
            if matches!(character, '%' | '/' | '\\') || character.is_ascii_control() {
                let _ = write!(name, "%{:02X}", u32::from(character));
            } else {
                name.push(character);
            }
        }
        for &byte in part.invalid() {
            let _ = write!(name, "%{byte:02X}");
        }
    }
    name
}

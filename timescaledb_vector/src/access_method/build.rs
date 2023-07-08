use pgrx::pg_sys::{BufferGetBlock, BufferGetBlockNumber, Pointer};
use pgrx::*;

use crate::util::page;
use crate::util::*;

const TSV_MAGIC_NUMBER: u32 = 768756476; //Magic number, random
const TSV_VERSION: u32 = 1;

/// This is metadata about the entire index.
/// Stored as the first page in the index relation.
struct TsvMetaPage {
    /// random magic number for identifying the index
    magic_number: u32,
    /// version number for future-proofing
    version: u32,
    /// number of dimensions in the vector
    num_dimensions: u32,
    /// max number of outgoing edges a node in the graph can have (R in the papers)
    num_neighbors: u32,
}

struct BuildState {
    memcxt: PgMemoryContexts,
    ntuples: usize,
}

impl BuildState {
    fn new() -> Self {
        BuildState {
            memcxt: PgMemoryContexts::new("tsv build context"),
            ntuples: 0,
        }
    }
}

/// Returns the TsvMetaPage from a page.
/// Should only be called from the very first page in a relation.
unsafe fn page_get_meta(page: pg_sys::Page, buffer: pg_sys::Buffer, new: bool) -> *mut TsvMetaPage {
    assert_eq!(BufferGetBlockNumber(buffer), 0);
    let meta_page = ports::PageGetContents(page) as *mut TsvMetaPage;
    if !new {
        assert_eq!((*meta_page).magic_number, TSV_MAGIC_NUMBER);
    }
    meta_page
}

/// Write out a new meta page.
/// Has to be done as the first write to a new relation.
unsafe fn write_meta_page(index: pg_sys::Relation, num_dimensions: u32, num_neighbors: u32) {
    let page = page::WritablePage::new(index);
    let meta = page_get_meta(*page, *(*(page.get_buffer())), true);
    (*meta).magic_number = TSV_MAGIC_NUMBER;
    (*meta).version = TSV_VERSION;
    (*meta).num_dimensions = num_dimensions;
    (*meta).num_neighbors = num_neighbors;
    let header = page.cast::<pgrx::pg_sys::PageHeaderData>();

    let meta_end = (meta as Pointer).add(std::mem::size_of::<TsvMetaPage>());
    let page_start = (*page) as Pointer;
    (*header).pd_lower = meta_end.offset_from(page_start) as _;

    page.commit();
}

#[pg_guard]
pub extern "C" fn ambuild(
    heaprel: pg_sys::Relation,
    indexrel: pg_sys::Relation,
    index_info: *mut pg_sys::IndexInfo,
) -> *mut pg_sys::IndexBuildResult {
    let heap_relation = unsafe { PgRelation::from_pg(heaprel) };
    let index_relation = unsafe { PgRelation::from_pg(indexrel) };
    unsafe { write_meta_page(indexrel, 1538, 50) };

    let ntuples = do_heap_scan(index_info, &heap_relation, &index_relation);

    let mut result = unsafe { PgBox::<pg_sys::IndexBuildResult>::alloc0() };
    result.heap_tuples = ntuples as f64;
    result.index_tuples = ntuples as f64;

    result.into_pg()
}

#[pg_guard]
pub unsafe extern "C" fn aminsert(
    index_relation: pg_sys::Relation,
    values: *mut pg_sys::Datum,
    _isnull: *mut bool,
    heap_tid: pg_sys::ItemPointer,
    _heap_relation: pg_sys::Relation,
    _check_unique: pg_sys::IndexUniqueCheck,
    _index_unchanged: bool,
    _index_info: *mut pg_sys::IndexInfo,
) -> bool {
    panic!("Not yet implemented")
}

#[pg_guard]
pub extern "C" fn ambuildempty(_index_relation: pg_sys::Relation) {
    panic!("Not yet implemented")
}

fn do_heap_scan<'a>(
    index_info: *mut pg_sys::IndexInfo,
    heap_relation: &'a PgRelation,
    index_relation: &'a PgRelation,
) -> usize {
    let mut state = BuildState::new();

    unsafe {
        pg_sys::IndexBuildHeapScan(
            heap_relation.as_ptr(),
            index_relation.as_ptr(),
            index_info,
            Some(build_callback),
            &mut state,
        );
    }

    let ntuples = state.ntuples;

    warning!("Indexed {} tuples", ntuples);
    ntuples
}

#[pg_guard]
unsafe extern "C" fn build_callback(
    _index: pg_sys::Relation,
    ctid: pg_sys::ItemPointer,
    values: *mut pg_sys::Datum,
    _isnull: *mut bool,
    _tuple_is_alive: bool,
    state: *mut std::os::raw::c_void,
) {
    build_callback_internal(*ctid, values, state);
}

#[inline(always)]
unsafe extern "C" fn build_callback_internal(
    ctid: pg_sys::ItemPointerData,
    values: *mut pg_sys::Datum,
    state: *mut std::os::raw::c_void,
) {
    check_for_interrupts!();

    let state = (state as *mut BuildState).as_mut().unwrap();
    let mut old_context = state.memcxt.set_as_current();

    state.ntuples = state.ntuples + 1;
    let values = std::slice::from_raw_parts(values, 1);

    old_context.set_as_current();
    state.memcxt.reset();
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::*;

    #[pg_test]
    unsafe fn test_index_creation() -> spi::Result<()> {
        Spi::run(&format!(
            "CREATE TABLE test(embedding vector(3));

            INSERT INTO test(embedding) VALUES ('[1,2,3]'), ('[4,5,6]');

            CREATE INDEX idxtest
                  ON test
               USING tsv(embedding)
                WITH (placeholder=30);",
        ))?;
        Ok(())
    }
}

//! [`MapSource`] over the store's bound reads.

use phantom_fractal_store::{CochangeGroup, MapFile, MapRow, Store, StoreError};

use super::MapSource;

impl MapSource for Store {
    fn map_files(&self) -> Result<Vec<MapFile>, StoreError> {
        self.files()
    }

    fn map_rows(&self) -> Result<Vec<MapRow>, StoreError> {
        Store::map_rows(self)
    }

    fn rank_epoch(&self) -> Result<u64, StoreError> {
        Store::rank_epoch(self)
    }

    fn stored_rank(&self) -> Result<Vec<(String, i64, f64)>, StoreError> {
        Store::stored_rank(self)
    }

    fn scan_head(&self) -> Result<Option<String>, StoreError> {
        self.cochange_head()
    }

    fn partners(
        &self,
        paths: &[String],
        min_lift: f64,
        per_file: u32,
    ) -> Result<Vec<CochangeGroup>, StoreError> {
        self.cochange_partners_for_files(paths, min_lift, per_file)
    }
}

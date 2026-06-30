pub mod desktop;
pub mod files;

use crate::core::PickerItem;
use crate::mru::MruCache;

pub trait Provider {
    fn fetch(&self, query: &str, mru: &MruCache) -> Vec<PickerItem>;
}

pub mod desktop;
pub mod desktop_spec;
pub mod files;

use crate::core::PickerItem;
use crate::mru::MruCache;
use fuzzy_matcher::skim::SkimMatcherV2;

pub trait Provider {
    fn fetch(&self, query: &str, mru: &MruCache, matcher: &SkimMatcherV2) -> Vec<PickerItem>;
}

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PickerItem {
    pub text: String,
    pub path: String,
    pub icon: String,
    pub is_dir: bool,
    pub is_submenu: bool,
    pub target_source: Option<String>,
    pub score: i64,
}

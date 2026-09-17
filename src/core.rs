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
    /// Pre-resolved argument vector for launching a desktop application.
    ///
    /// When present (desktop entries only), the execute path spawns this argv
    /// directly instead of routing the raw string through a shell. This avoids
    /// `sh -c` quoting/injection issues and implements the desktop-entry
    /// field-code rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_argv: Option<Vec<String>>,
}

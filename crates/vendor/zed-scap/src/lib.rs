pub mod capturer;
pub mod frame;
mod targets;
mod utils;

pub use targets::get_all_targets;
pub use targets::Target;
pub use targets::{Display, Window};
pub use utils::has_permission;
pub use utils::is_supported;
pub use utils::request_permission;

#[cfg(target_os = "macos")]
pub mod engine {
    pub use crate::capturer::engine::mac;
}

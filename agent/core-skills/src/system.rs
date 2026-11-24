pub(crate) use forall_skills::install_system_skills;
pub(crate) use forall_skills::system_cache_root_dir;

use forall_utils_absolute_path::AbsolutePathBuf;

pub(crate) fn uninstall_system_skills(forall_home: &AbsolutePathBuf) {
    let _ = std::fs::remove_dir_all(system_cache_root_dir(forall_home));
}

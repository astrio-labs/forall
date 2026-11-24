use forall_core::config::Config;
use forall_login::AuthManager;

pub(crate) async fn rate_limits_ok(_auth_manager: &AuthManager, _config: &Config) -> bool {
    true
}

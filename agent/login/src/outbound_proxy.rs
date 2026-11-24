/// Auth-layer marker for client-owned proxy policy.
///
/// `AuthConfig` carries this value while endpoint resolution and platform details remain in the
/// client layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRouteConfig {}

impl AuthRouteConfig {
    pub fn respect_system_proxy() -> Self {
        Self {}
    }
}

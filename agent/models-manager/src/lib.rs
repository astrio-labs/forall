pub(crate) mod cache;
pub mod collaboration_mode_presets;
pub(crate) mod config;
mod forall_branding;
pub mod manager;
pub mod model_info;
pub mod model_presets;
pub mod openai_compatible_catalog;
pub(crate) mod openai_model_ranking;
pub mod test_support;

pub use config::ModelsManagerConfig;
pub use forall_app_server_protocol::AuthMode;

fn apply_forall_branding(model: &mut forall_protocol::openai_models::ModelInfo) {
    model.base_instructions =
        forall_branding::rebrand_instructions(crate::model_info::BASE_INSTRUCTIONS);
    model.model_messages = None;
}

/// Load the bundled model catalog shipped with `forall-models-manager`.
pub fn bundled_models_response()
-> std::result::Result<forall_protocol::openai_models::ModelsResponse, serde_json::Error> {
    let mut response: forall_protocol::openai_models::ModelsResponse =
        serde_json::from_str(include_str!("../models.json"))?;
    for model in &mut response.models {
        apply_forall_branding(model);
    }
    Ok(response)
}

/// Convert the client version string to a whole version string (e.g. "1.2.3-alpha.4" -> "1.2.3").
pub fn client_version_to_whole() -> String {
    format!(
        "{}.{}.{}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH")
    )
}

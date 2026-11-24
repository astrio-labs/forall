use forall_config::CONFIG_TOML_FILE;
use forall_config::ConfigLayerStack;
use forall_config::TomlValue;
use forall_core::config::Config;
use forall_features::Feature;
use forall_hooks::HookListEntry;
use forall_utils_absolute_path::AbsolutePathBuf;

pub fn trust_discovered_hooks(config: &mut Config) {
    config
        .features
        .enable(Feature::ForallHooks)
        .expect("test config should allow feature update");

    let listed = forall_hooks::list_hooks(forall_hooks::HooksConfig {
        feature_enabled: true,
        config_layer_stack: Some(config.config_layer_stack.clone()),
        ..forall_hooks::HooksConfig::default()
    });
    assert!(
        !listed.hooks.is_empty(),
        "trusted hook fixture should discover at least one hook"
    );
    trust_hooks(config, listed.hooks);
}

pub fn trust_hooks(config: &mut Config, hooks: Vec<HookListEntry>) {
    config.config_layer_stack =
        trusted_config_layer_stack(&config.config_layer_stack, &config.forall_home, hooks);
}

pub fn trusted_config_layer_stack(
    config_layer_stack: &ConfigLayerStack,
    forall_home: &AbsolutePathBuf,
    hooks: Vec<HookListEntry>,
) -> ConfigLayerStack {
    let mut user_config = config_layer_stack
        .get_active_user_layer()
        .map(|layer| layer.config.clone())
        .unwrap_or_else(|| TomlValue::Table(Default::default()));
    let user_table = user_config
        .as_table_mut()
        .expect("user config should be a table");
    let hooks_table = user_table
        .entry("hooks")
        .or_insert_with(|| TomlValue::Table(Default::default()))
        .as_table_mut()
        .expect("hooks config should be a table");
    let state_table = hooks_table
        .entry("state")
        .or_insert_with(|| TomlValue::Table(Default::default()))
        .as_table_mut()
        .expect("hook state config should be a table");
    for hook in hooks {
        let mut hook_state = TomlValue::Table(Default::default());
        let hook_state_table = hook_state
            .as_table_mut()
            .expect("hook state should be a table");
        hook_state_table.insert(
            "trusted_hash".to_string(),
            TomlValue::String(hook.current_hash),
        );
        state_table.insert(hook.key, hook_state);
    }

    config_layer_stack.with_user_config(&forall_home.join(CONFIG_TOML_FILE), user_config)
}

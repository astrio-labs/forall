use super::*;
use crate::McpPluginAttribution;
use crate::McpServerRegistration;
use forall_config::Constrained;
use forall_config::types::AppToolApproval;
use forall_config::types::AuthKeyringBackendKind;
use forall_login::ForallAuth;
use forall_plugin::PluginCapabilitySummary;
use forall_protocol::models::ManagedFileSystemPermissions;
use forall_protocol::models::PermissionProfile;
use forall_protocol::permissions::NetworkSandboxPolicy;
use forall_protocol::protocol::AskForApproval;
use forall_protocol::protocol::GranularApprovalConfig;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

fn test_mcp_config(forall_home: PathBuf) -> McpConfig {
    McpConfig {
        forall_home,
        mcp_oauth_credentials_store_mode: OAuthCredentialsStoreMode::default(),
        auth_keyring_backend_kind: AuthKeyringBackendKind::default(),
        mcp_oauth_callback_port: None,
        mcp_oauth_callback_url: None,
        skill_mcp_dependency_install_enabled: true,
        approval_policy: Constrained::allow_any(AskForApproval::OnFailure),
        forall_linux_sandbox_exe: None,
        use_legacy_landlock: false,
        prefix_mcp_tool_names: true,
        client_elicitation_capability: ElicitationCapability::default(),
        mcp_server_catalog: ResolvedMcpCatalog::default(),
        plugin_capability_summaries: Vec::new(),
    }
}

fn test_streamable_http_mcp_server_config(url: &str) -> McpServerConfig {
    McpServerConfig {
        transport: McpServerTransportConfig::StreamableHttp {
            url: url.to_string(),
            bearer_token_env_var: None,
            http_headers: None,
            env_http_headers: None,
        },
        environment_id: forall_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
        enabled: true,
        required: false,
        supports_parallel_tool_calls: false,
        disabled_reason: None,
        startup_timeout_sec: Some(Duration::from_secs(30)),
        tool_timeout_sec: None,
        default_tools_approval_mode: None,
        enabled_tools: None,
        disabled_tools: None,
        scopes: None,
        oauth: None,
        oauth_resource: None,
        tools: HashMap::new(),
    }
}

#[test]
fn qualified_mcp_tool_name_prefix_sanitizes_server_names_without_lowercasing() {
    assert_eq!(
        qualified_mcp_tool_name_prefix("Some-Server"),
        "mcp__Some_Server__".to_string()
    );
}

#[test]
fn mcp_prompt_auto_approval_honors_unrestricted_managed_profiles() {
    assert!(mcp_permission_prompt_is_auto_approved(
        AskForApproval::Never,
        &PermissionProfile::Managed {
            file_system: ManagedFileSystemPermissions::Unrestricted,
            network: NetworkSandboxPolicy::Enabled,
        },
        McpPermissionPromptAutoApproveContext::default(),
    ));
    assert!(mcp_permission_prompt_is_auto_approved(
        AskForApproval::Never,
        &PermissionProfile::Managed {
            file_system: ManagedFileSystemPermissions::Unrestricted,
            network: NetworkSandboxPolicy::Restricted,
        },
        McpPermissionPromptAutoApproveContext::default(),
    ));
    assert!(!mcp_permission_prompt_is_auto_approved(
        AskForApproval::Never,
        &PermissionProfile::read_only(),
        McpPermissionPromptAutoApproveContext::default(),
    ));
    assert!(!mcp_permission_prompt_is_auto_approved(
        AskForApproval::OnRequest,
        &PermissionProfile::Managed {
            file_system: ManagedFileSystemPermissions::Unrestricted,
            network: NetworkSandboxPolicy::Enabled,
        },
        McpPermissionPromptAutoApproveContext::default(),
    ));
}

#[test]
fn mcp_prompt_auto_approval_honors_approved_tools_in_all_permission_modes() {
    for approval_policy in [
        AskForApproval::UnlessTrusted,
        AskForApproval::OnFailure,
        AskForApproval::OnRequest,
        AskForApproval::Granular(GranularApprovalConfig {
            sandbox_approval: true,
            rules: true,
            skill_approval: true,
            request_permissions: true,
            mcp_elicitations: true,
        }),
        AskForApproval::Never,
    ] {
        assert!(mcp_permission_prompt_is_auto_approved(
            approval_policy,
            &PermissionProfile::read_only(),
            McpPermissionPromptAutoApproveContext {
                tool_approval_mode: Some(AppToolApproval::Approve),
            },
        ));
    }

    assert!(!mcp_permission_prompt_is_auto_approved(
        AskForApproval::OnRequest,
        &PermissionProfile::read_only(),
        McpPermissionPromptAutoApproveContext {
            tool_approval_mode: Some(AppToolApproval::Auto),
        },
    ));
}

#[test]
fn mcp_prompt_auto_approval_rejects_auto_mode_in_default_permission_mode() {
    assert!(!mcp_permission_prompt_is_auto_approved(
        AskForApproval::OnRequest,
        &PermissionProfile::read_only(),
        McpPermissionPromptAutoApproveContext {
            tool_approval_mode: Some(AppToolApproval::Auto),
        },
    ));
}

#[test]
fn tool_plugin_provenance_collects_mcp_sources() {
    let mut config = test_mcp_config(PathBuf::new());
    let mut catalog = ResolvedMcpCatalog::builder();
    catalog.register(McpServerRegistration::from_plugin(
        "alpha".to_string(),
        McpPluginAttribution::new("alpha@test".to_string(), "alpha-plugin".to_string()),
        /*plugin_order*/ 0,
        test_streamable_http_mcp_server_config("https://alpha.example/mcp"),
    ));
    config.mcp_server_catalog = catalog.build();
    config.plugin_capability_summaries = vec![
        PluginCapabilitySummary {
            config_name: "alpha@test".to_string(),
            display_name: "alpha-plugin".to_string(),
            mcp_server_names: vec!["alpha".to_string()],
            ..PluginCapabilitySummary::default()
        },
        PluginCapabilitySummary {
            config_name: "beta@test".to_string(),
            display_name: "beta-plugin".to_string(),
            mcp_server_names: vec!["beta".to_string()],
            ..PluginCapabilitySummary::default()
        },
    ];
    let provenance = tool_plugin_provenance(&config);

    assert_eq!(
        provenance,
        ToolPluginProvenance {
            plugin_display_names_by_connector_id: HashMap::new(),
            plugin_display_names_by_mcp_server_name: HashMap::from([(
                "alpha".to_string(),
                vec!["alpha-plugin".to_string()],
            )]),
            plugin_ids_by_mcp_server_name: HashMap::from([(
                "alpha".to_string(),
                "alpha@test".to_string(),
            )]),
            selected_plugin_mcp_server_names: HashSet::new(),
        }
    );
    assert_eq!(
        provenance.plugin_id_for_mcp_server_name("alpha"),
        Some("alpha@test")
    );
    assert_eq!(provenance.plugin_id_for_mcp_server_name("beta"), None);
}

#[test]
fn selected_mcp_attribution_does_not_join_an_unrelated_local_summary() {
    let mut config = test_mcp_config(PathBuf::new());
    let mut catalog = ResolvedMcpCatalog::builder();
    catalog.register(McpServerRegistration::from_selected_plugin(
        "github".to_string(),
        McpPluginAttribution::new(
            "shared-plugin-id".to_string(),
            "Executor GitHub".to_string(),
        ),
        /*selection_order*/ 0,
        test_streamable_http_mcp_server_config("https://github.example/mcp"),
    ));
    config.mcp_server_catalog = catalog.build();
    config.plugin_capability_summaries = vec![PluginCapabilitySummary {
        config_name: "shared-plugin-id".to_string(),
        display_name: "Local GitHub".to_string(),
        mcp_server_names: vec!["github".to_string()],
        ..PluginCapabilitySummary::default()
    }];

    let provenance = tool_plugin_provenance(&config);

    assert_eq!(
        provenance,
        ToolPluginProvenance {
            plugin_display_names_by_connector_id: HashMap::new(),
            plugin_display_names_by_mcp_server_name: HashMap::from([(
                "github".to_string(),
                vec!["Executor GitHub".to_string()],
            )]),
            plugin_ids_by_mcp_server_name: HashMap::from([(
                "github".to_string(),
                "shared-plugin-id".to_string(),
            )]),
            selected_plugin_mcp_server_names: HashSet::from(["github".to_string()]),
        }
    );
    assert!(provenance.is_selected_plugin_mcp_server("github"));
}

#[test]
fn playwright_mcp_server_config_uses_stdio_npx() {
    let config = playwright_mcp_server_config();
    match &config.transport {
        McpServerTransportConfig::Stdio { command, args, .. } => {
            assert_eq!(command, "npx");
            assert_eq!(args, &["-y", "@playwright/mcp@latest"]);
        }
        other => panic!("expected stdio transport for playwright, got {other:?}"),
    }
    assert!(config.enabled);
    assert_eq!(config.startup_timeout_sec, Some(Duration::from_secs(60)));
}

#[tokio::test]
async fn effective_mcp_servers_preserve_runtime_servers() {
    let forall_home = tempfile::tempdir().expect("tempdir");
    let mut config = test_mcp_config(forall_home.path().to_path_buf());
    let auth = ForallAuth::create_dummy_api_key_auth_for_testing();

    let mut catalog = ResolvedMcpCatalog::builder();
    catalog.register(McpServerRegistration::from_config(
        "sample".to_string(),
        test_streamable_http_mcp_server_config("https://user.example/mcp"),
    ));
    catalog.register(McpServerRegistration::from_config(
        "docs".to_string(),
        test_streamable_http_mcp_server_config("https://docs.example/mcp"),
    ));
    config.mcp_server_catalog = catalog.build();

    let effective = effective_mcp_servers(&config, Some(&auth));

    let sample = effective.get("sample").expect("user server should exist");
    let docs = effective
        .get("docs")
        .expect("configured server should exist");

    let sample = sample
        .configured_config()
        .expect("configured server should retain transport");
    let docs = docs
        .configured_config()
        .expect("configured server should retain transport");

    match &sample.transport {
        McpServerTransportConfig::StreamableHttp { url, .. } => {
            assert_eq!(url, "https://user.example/mcp");
        }
        other => panic!("expected streamable http transport, got {other:?}"),
    }
    match &docs.transport {
        McpServerTransportConfig::StreamableHttp { url, .. } => {
            assert_eq!(url, "https://docs.example/mcp");
        }
        other => panic!("expected streamable http transport, got {other:?}"),
    }
}

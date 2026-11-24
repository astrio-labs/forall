//! Native local Forall authoring tools (namespace `author`).

mod spec;

use forall_workflow::authoring::{
    self, CanonicalRoot, ContractTarget, MutationMode, MutationOptions,
};
use forall_workflow::mapping::schema::{CodeRef, Requirement};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::PreToolUsePayload;
use crate::tools::registry::ToolExecutor;
use forall_tools::JsonToolOutput;
use forall_tools::ToolName;
use forall_tools::ToolSpec;

use spec::AUTHOR_NAMESPACE;

const MAPPING_PATH: &str = ".forall/verify/mapping.yaml";

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ApplyMode {
    Preview,
    Apply,
}

impl ApplyMode {
    fn mutation(self, expected_sha256: BTreeMap<String, String>) -> MutationOptions {
        MutationOptions {
            mode: match self {
                Self::Preview => MutationMode::Preview,
                Self::Apply => MutationMode::Apply,
            },
            expected_sha256,
        }
    }
}

#[derive(Debug, Deserialize)]
struct EmptyRequest {}

#[derive(Debug, Deserialize)]
struct InitRequest {
    mode: ApplyMode,
}

#[derive(Debug, Deserialize)]
struct DiscoverRequest {
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    languages: Vec<LanguageInput>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LanguageInput {
    TypeScript,
    Rust,
    Java,
    C,
}

#[derive(Debug, Clone, Deserialize)]
struct CodeRefInput {
    file: String,
    symbols: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RequirementInput {
    id: String,
    capability: String,
    requirement: String,
    #[serde(default)]
    verified: bool,
    #[serde(default)]
    property_tested: bool,
    code: Option<CodeRefInput>,
    contract: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpsertRequirementsRequest {
    mode: ApplyMode,
    expected_mapping_sha256: Option<String>,
    requirements: Vec<RequirementInput>,
}

#[derive(Debug, Clone, Deserialize)]
struct ContractInput {
    file: String,
    symbol: String,
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    ensures: Vec<String>,
    expected_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScaffoldContractsRequest {
    mode: ApplyMode,
    contracts: Vec<ContractInput>,
}

#[derive(Debug, Deserialize)]
struct ScaffoldPropertyRequest {
    mode: ApplyMode,
    requirement_id: String,
    body: String,
    symbol: Option<String>,
    expected_sha256: Option<String>,
    expected_mapping_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthorOperation {
    Status,
    Init,
    Discover,
    UpsertRequirements,
    ScaffoldContracts,
    Validate,
    ScaffoldProperty,
}

impl AuthorOperation {
    fn name(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Init => "init",
            Self::Discover => "discover",
            Self::UpsertRequirements => "upsert_requirements",
            Self::ScaffoldContracts => "scaffold_contracts",
            Self::Validate => "validate",
            Self::ScaffoldProperty => "scaffold_property",
        }
    }

    fn spec(self) -> ToolSpec {
        match self {
            Self::Status => spec::create_status_tool(),
            Self::Init => spec::create_init_tool(),
            Self::Discover => spec::create_discover_tool(),
            Self::UpsertRequirements => spec::create_upsert_requirements_tool(),
            Self::ScaffoldContracts => spec::create_scaffold_contracts_tool(),
            Self::Validate => spec::create_validate_tool(),
            Self::ScaffoldProperty => spec::create_scaffold_property_tool(),
        }
    }

    fn is_read_only(self) -> bool {
        matches!(self, Self::Status | Self::Discover | Self::Validate)
    }
}

pub struct AuthorHandler {
    operation: AuthorOperation,
}

impl AuthorHandler {
    const fn new(operation: AuthorOperation) -> Self {
        Self { operation }
    }

    pub const fn status() -> Self {
        Self::new(AuthorOperation::Status)
    }

    pub const fn init() -> Self {
        Self::new(AuthorOperation::Init)
    }

    pub const fn discover() -> Self {
        Self::new(AuthorOperation::Discover)
    }

    pub const fn upsert_requirements() -> Self {
        Self::new(AuthorOperation::UpsertRequirements)
    }

    pub const fn scaffold_contracts() -> Self {
        Self::new(AuthorOperation::ScaffoldContracts)
    }

    pub const fn validate() -> Self {
        Self::new(AuthorOperation::Validate)
    }

    pub const fn scaffold_property() -> Self {
        Self::new(AuthorOperation::ScaffoldProperty)
    }

    fn parse_input<T: for<'de> Deserialize<'de>>(
        invocation: &ToolInvocation,
    ) -> Result<T, FunctionCallError> {
        let ToolPayload::Function { arguments } = &invocation.payload else {
            return Err(FunctionCallError::RespondToModel(
                "author tool received unsupported payload".to_string(),
            ));
        };
        parse_arguments(arguments)
    }

    fn project_root(
        invocation: &ToolInvocation,
    ) -> Result<CanonicalRoot, authoring::AuthoringError> {
        let cwd = resolve_tool_environment(&invocation.turn, None)
            .map_err(|error| authoring_root_error(error.to_string()))?
            .ok_or_else(|| authoring_root_error("author tools require a turn environment"))?
            .cwd()
            .to_abs_path()
            .map_err(|error| {
                authoring_root_error(format!("failed to resolve project cwd: {error}"))
            })?;
        let root =
            forall_workflow::paths::find_project_root(&cwd).unwrap_or_else(|| cwd.to_path_buf());
        CanonicalRoot::new(root)
    }

    fn output<T: Serialize>(
        result: Result<T, authoring::AuthoringError>,
    ) -> Box<dyn crate::tools::context::ToolOutput> {
        match result {
            Ok(result) => Box::new(JsonToolOutput::new(serde_json::json!({
                "ok": true,
                "result": result,
            }))),
            Err(error) => Box::new(JsonToolOutput::with_success(
                serde_json::json!({
                    "ok": false,
                    "error": {
                        "code": error.code,
                        "message": error.message,
                        "path": error.path,
                    },
                }),
                Some(false),
            )),
        }
    }

    fn hook_payload(&self, invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        if self.operation.is_read_only() {
            return None;
        }
        let ToolPayload::Function { arguments } = &invocation.payload else {
            return None;
        };
        let mut input = serde_json::from_str::<Value>(arguments).ok()?;
        if input.get("mode").and_then(Value::as_str) != Some("apply") {
            return None;
        }
        let paths = match self.operation {
            AuthorOperation::Init => vec![
                MAPPING_PATH.to_string(),
                ".forall/workflow/config.yaml".to_string(),
                ".forall/AGENTS.md".to_string(),
            ],
            AuthorOperation::UpsertRequirements => vec![MAPPING_PATH.to_string()],
            AuthorOperation::ScaffoldContracts => input
                .get("contracts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|contract| contract.get("file").and_then(Value::as_str))
                .map(str::to_string)
                .collect(),
            AuthorOperation::ScaffoldProperty => {
                let requirement_id = input.get("requirement_id")?.as_str()?;
                vec![
                    MAPPING_PATH.to_string(),
                    format!(".forall/scenarios/{requirement_id}.property.ts"),
                ]
            }
            AuthorOperation::Status | AuthorOperation::Discover | AuthorOperation::Validate => {
                Vec::new()
            }
        };
        let Value::Object(input) = &mut input else {
            return None;
        };
        input.insert("paths".to_string(), serde_json::json!(paths));
        Some(PreToolUsePayload {
            tool_name: HookToolName::new(format!("author.{}", self.operation.name())),
            tool_input: Value::Object(input.clone()),
        })
    }

    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let root = match Self::project_root(&invocation) {
            Ok(root) => root,
            Err(error) => return Ok(Self::output::<Value>(Err(error))),
        };
        self.execute_at_root(&root, &invocation)
    }

    fn execute_at_root(
        &self,
        root: &CanonicalRoot,
        invocation: &ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let output = match self.operation {
            AuthorOperation::Status => {
                let _: EmptyRequest = Self::parse_input(invocation)?;
                Self::output(authoring::project_status(root))
            }
            AuthorOperation::Init => {
                let input: InitRequest = Self::parse_input(invocation)?;
                Self::output(authoring::init_project(
                    root,
                    &input.mode.mutation(BTreeMap::new()),
                ))
            }
            AuthorOperation::Discover => {
                let input: DiscoverRequest = Self::parse_input(invocation)?;
                let result = if input.files.is_empty() {
                    let languages = input
                        .languages
                        .into_iter()
                        .map(|language| match language {
                            LanguageInput::TypeScript => authoring::SourceLanguage::TypeScript,
                            LanguageInput::Rust => authoring::SourceLanguage::Rust,
                            LanguageInput::Java => authoring::SourceLanguage::Java,
                            LanguageInput::C => authoring::SourceLanguage::C,
                        })
                        .collect::<Vec<_>>();
                    authoring::discover_project_symbols(root, &languages)
                } else {
                    authoring::discover_symbols(root, &input.files)
                };
                Self::output(result)
            }
            AuthorOperation::UpsertRequirements => {
                let input: UpsertRequirementsRequest = Self::parse_input(invocation)?;
                let mut expected = BTreeMap::new();
                if let Some(hash) = input.expected_mapping_sha256 {
                    expected.insert(MAPPING_PATH.to_string(), hash);
                }
                let operation = authoring::UpsertRequirementsRequest {
                    requirements: input
                        .requirements
                        .into_iter()
                        .map(requirement_from_input)
                        .collect(),
                    mutation: input.mode.mutation(expected),
                };
                Self::output(authoring::upsert_requirements(root, &operation))
            }
            AuthorOperation::ScaffoldContracts => {
                let input: ScaffoldContractsRequest = Self::parse_input(invocation)?;
                let mut expected = BTreeMap::new();
                let contracts = input
                    .contracts
                    .into_iter()
                    .map(|contract| {
                        if let Some(hash) = contract.expected_sha256 {
                            expected.insert(contract.file.clone(), hash);
                        }
                        ContractTarget {
                            file: contract.file,
                            symbol: contract.symbol,
                            requires: contract.requires,
                            ensures: contract.ensures,
                        }
                    })
                    .collect();
                let operation = authoring::ScaffoldContractsRequest {
                    contracts,
                    mutation: input.mode.mutation(expected),
                };
                Self::output(authoring::scaffold_contracts(root, &operation))
            }
            AuthorOperation::Validate => {
                let _: EmptyRequest = Self::parse_input(invocation)?;
                Self::output(authoring::validate_authoring(root))
            }
            AuthorOperation::ScaffoldProperty => {
                let input: ScaffoldPropertyRequest = Self::parse_input(invocation)?;
                let mut expected = BTreeMap::new();
                if let Some(hash) = input.expected_sha256 {
                    expected.insert(
                        format!(".forall/scenarios/{}.property.ts", input.requirement_id),
                        hash,
                    );
                }
                if let Some(hash) = input.expected_mapping_sha256 {
                    expected.insert(MAPPING_PATH.to_string(), hash);
                }
                let operation = authoring::ScaffoldPropertyRequest {
                    requirement_id: input.requirement_id,
                    body: input.body,
                    symbol: input.symbol,
                    mutation: input.mode.mutation(expected),
                };
                Self::output(authoring::scaffold_property(root, &operation))
            }
        };
        Ok(output)
    }
}

impl ToolExecutor<ToolInvocation> for AuthorHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(AUTHOR_NAMESPACE, self.operation.name())
    }

    fn spec(&self) -> ToolSpec {
        self.operation.spec()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        self.operation.is_read_only()
    }

    fn handle(&self, invocation: ToolInvocation) -> forall_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl CoreToolRuntime for AuthorHandler {
    fn pre_tool_use_payload(&self, invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        self.hook_payload(invocation)
    }
}

fn requirement_from_input(input: RequirementInput) -> Requirement {
    Requirement {
        id: input.id,
        capability: input.capability,
        requirement: input.requirement,
        verified: input.verified,
        property_tested: input.property_tested,
        property: None,
        code: input.code.map(|code| CodeRef {
            file: code.file,
            symbols: code.symbols,
        }),
        contract: input.contract,
        claimcheck: None,
        scenarios: None,
    }
}

fn authoring_root_error(message: impl Into<String>) -> authoring::AuthoringError {
    authoring::AuthoringError {
        code: authoring::AuthoringErrorCode::InvalidRoot,
        message: message.into(),
        path: None,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

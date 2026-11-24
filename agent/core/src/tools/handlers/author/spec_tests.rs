use super::*;

#[test]
fn author_tool_specs_snapshot() {
    let snapshot = [
        create_status_tool(),
        create_init_tool(),
        create_discover_tool(),
        create_upsert_requirements_tool(),
        create_scaffold_contracts_tool(),
        create_validate_tool(),
        create_scaffold_property_tool(),
    ]
    .into_iter()
    .map(|spec| {
        let ToolSpec::Namespace(namespace) = spec else {
            panic!("author tools must be namespaced");
        };
        let ResponsesApiNamespaceTool::Function(tool) = &namespace.tools[0];
        let properties = tool
            .parameters
            .properties
            .as_ref()
            .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        format!(
            "{}|required={:?}|properties={properties:?}|output={}",
            tool.name,
            tool.parameters.required.as_deref().unwrap_or_default(),
            tool.output_schema.is_some()
        )
    })
    .collect::<Vec<_>>()
    .join("\n");

    insta::assert_snapshot!(snapshot, @r###"
    status|required=[]|properties=[]|output=true
    init|required=["mode"]|properties=["mode"]|output=true
    discover|required=[]|properties=["files", "languages"]|output=true
    upsert_requirements|required=["mode", "requirements"]|properties=["expected_mapping_sha256", "mode", "requirements"]|output=true
    scaffold_contracts|required=["mode", "contracts"]|properties=["contracts", "mode"]|output=true
    validate|required=[]|properties=[]|output=true
    scaffold_property|required=["mode", "requirement_id", "body"]|properties=["body", "expected_mapping_sha256", "expected_sha256", "mode", "requirement_id", "symbol"]|output=true
    "###);
}

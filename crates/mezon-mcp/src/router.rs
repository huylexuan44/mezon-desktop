//! Shared MCP tool router built from [`crate::catalog::TOOL_SPECS`].

use crate::catalog::TOOL_SPECS;
use crate::schemas;
use rmcp::{
    ErrorData,
    handler::server::router::tool::{ToolRoute, ToolRouter},
    model::{CallToolResult, Tool},
};
use serde_json::{Map, Value};
use std::{future::Future, sync::Arc};

pub fn json_object_schema() -> Arc<Map<String, Value>> {
    Arc::new(
        serde_json::from_value(serde_json::json!({
            "type": "object",
            "additionalProperties": true
        }))
        .unwrap_or_default(),
    )
}

pub fn tool_call_result(value: Value) -> Result<CallToolResult, String> {
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    Ok(CallToolResult::success(vec![
        rmcp::model::ContentBlock::text(text),
    ]))
}

pub fn server_instructions(read_only: bool) -> String {
    let mode = if read_only { "read-only" } else { "read/write" };
    format!(
        "Mezon desktop MCP tools ({mode}). Requires the Mezon app to be running and signed in.\n\n\
         Workflow:\n\
         1. get_current_context — discover route, clan_id, channel_id\n\
         2. list_messages / get_message — read chat; search_messages to find text\n\
         3. send_message — needs clan_id, channel_id, content (use clan_id=0 for DMs)\n\
         4. capture_chat / capture_window — need Screen Recording permission; save data_base64 to a file, then send_image with path\n\
         5. get_message before click_message_button or select_message_option\n\n\
         Snowflake ids accept integer or numeric string. Write tools return an explicit read-only error when disabled."
    )
}

pub fn tool_count(read_only: bool) -> usize {
    TOOL_SPECS
        .iter()
        .filter(|spec| !read_only || !spec.write)
        .count()
}

pub fn build_tool_router<S, F, Fut>(read_only: bool, invoke: F) -> ToolRouter<S>
where
    S: Send + Sync + 'static,
    F: Fn(String, Value) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<Value>> + Send + 'static,
{
    let mut router = ToolRouter::<S>::new();
    for spec in TOOL_SPECS.iter().filter(|spec| !read_only || !spec.write) {
        let tool_name = spec.name.to_string();
        let invoke = invoke.clone();
        router.add_route(ToolRoute::new_dyn(
            Tool::new(
                spec.name,
                spec.description,
                schemas::input_schema(spec.name),
            ),
            move |mut ctx| {
                let tool_name = tool_name.clone();
                let invoke = invoke.clone();
                Box::pin(async move {
                    let args = ctx
                        .arguments
                        .take()
                        .map(Value::Object)
                        .unwrap_or(Value::Null);
                    invoke(tool_name, args)
                        .await
                        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))
                        .and_then(|value| {
                            tool_call_result(value).map_err(|e| ErrorData::invalid_params(e, None))
                        })
                })
            },
        ));
    }
    router
}

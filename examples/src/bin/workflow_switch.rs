use ows_runtime_examples::{run_workflow_with_input, ExampleResult};
use serde_json::json;

#[tokio::main]
async fn main() -> ExampleResult<()> {
    run_workflow_with_input("workflows/switch.yaml", json!({ "ready": true })).await
}

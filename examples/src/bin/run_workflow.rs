use ows_runtime_examples::{execute_yaml, print_output, runtime, ExampleResult};
use serde_json::json;

#[tokio::main]
async fn main() -> ExampleResult<()> {
    let output = execute_yaml(
        &runtime()?,
        "workflows/run_workflow.yaml",
        json!({ "name": "World" }),
    )
    .await?;
    print_output(&output)
}

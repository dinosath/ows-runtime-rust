use ows_runtime_examples::{run_network_workflow, ExampleResult};

#[tokio::main]
async fn main() -> ExampleResult<()> {
    run_network_workflow("workflows/call.yaml").await
}

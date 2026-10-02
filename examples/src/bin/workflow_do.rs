use ows_runtime_examples::{run_workflow, ExampleResult};

#[tokio::main]
async fn main() -> ExampleResult<()> {
    run_workflow("workflows/do.yaml").await
}

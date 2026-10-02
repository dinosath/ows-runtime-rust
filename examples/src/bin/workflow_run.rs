use ows_runtime_examples::{run_script_workflow, ExampleResult};

#[tokio::main]
async fn main() -> ExampleResult<()> {
    run_script_workflow("workflows/run.yaml").await
}

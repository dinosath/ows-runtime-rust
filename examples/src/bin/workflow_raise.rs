use ows_runtime_examples::{run_expected_fault, ExampleResult};

#[tokio::main]
async fn main() -> ExampleResult<()> {
    run_expected_fault("workflows/raise.yaml").await
}

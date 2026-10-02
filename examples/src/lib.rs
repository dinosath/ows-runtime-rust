use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ows_runtime::{Runtime, TokioProcessRunner, Workflow};
use ows_runtime_core::{EventMessage, EventPublisher, RuntimePolicy};
use ows_runtime_events::InMemoryBroker;
use serde_json::{json, Value};

pub type ExampleResult<T> = Result<T, Box<dyn Error>>;

pub fn workflow_path(relative: impl AsRef<Path>) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

pub fn load_yaml(relative: impl AsRef<Path>) -> ExampleResult<Workflow> {
    Ok(Workflow::from_yaml(&std::fs::read_to_string(
        workflow_path(relative),
    )?)?)
}

pub fn load_json(relative: impl AsRef<Path>) -> ExampleResult<Workflow> {
    Ok(Workflow::from_json(&std::fs::read_to_string(
        workflow_path(relative),
    )?)?)
}

pub fn runtime() -> ExampleResult<Runtime> {
    Ok(Runtime::builder().build()?)
}

pub fn network_runtime() -> ExampleResult<Runtime> {
    Ok(Runtime::builder()
        .with_policy(RuntimePolicy {
            allow_network: true,
            ..Default::default()
        })
        .build()?)
}

pub fn script_runtime() -> ExampleResult<Runtime> {
    Ok(Runtime::builder()
        .with_process(Arc::new(TokioProcessRunner::new().allow_scripts()))
        .build()?)
}

pub async fn execute(runtime: &Runtime, workflow: &Workflow, input: Value) -> ExampleResult<Value> {
    Ok(runtime.run(workflow, input).await?)
}

pub async fn execute_yaml(
    runtime: &Runtime,
    relative: impl AsRef<Path>,
    input: Value,
) -> ExampleResult<Value> {
    let workflow = load_yaml(relative)?;
    execute(runtime, &workflow, input).await
}

pub async fn run_workflow(relative: impl AsRef<Path>) -> ExampleResult<()> {
    run_workflow_with_input(relative, Value::Null).await
}

pub async fn run_workflow_with_input(
    relative: impl AsRef<Path>,
    input: Value,
) -> ExampleResult<()> {
    let output = execute_yaml(&runtime()?, relative, input).await?;
    print_output(&output)
}

pub async fn run_network_workflow(relative: impl AsRef<Path>) -> ExampleResult<()> {
    let output = execute_yaml(&network_runtime()?, relative, Value::Null).await?;
    print_output(&output)
}

pub async fn run_script_workflow(relative: impl AsRef<Path>) -> ExampleResult<()> {
    let output = execute_yaml(&script_runtime()?, relative, Value::Null).await?;
    print_output(&output)
}

pub async fn run_expected_fault(relative: impl AsRef<Path>) -> ExampleResult<()> {
    match execute_yaml(&runtime()?, relative, Value::Null).await {
        Ok(_) => Err("workflow unexpectedly completed".into()),
        Err(error) => {
            println!("expected workflow fault: {error}");
            Ok(())
        }
    }
}

pub fn print_output(output: &Value) -> ExampleResult<()> {
    println!("{}", serde_json::to_string_pretty(output)?);
    Ok(())
}

pub async fn execute_listen() -> ExampleResult<Value> {
    let broker = InMemoryBroker::new();
    let runtime = Runtime::builder()
        .with_event_publisher(Arc::new(broker.clone()))
        .with_event_consumer(Arc::new(broker.clone()))
        .build()?;
    let workflow = load_yaml("workflows/listen.yaml")?;
    let publisher = broker.clone();
    let publish_input = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        publisher
            .publish(
                &EventMessage::new(
                    "workflow-listen-input",
                    "https://open-workflow-specification.org/examples",
                    "examples.node-types.input",
                )
                .with_data(json!({ "message": "hello from listen" })),
            )
            .await
    });
    let output = execute(&runtime, &workflow, json!({})).await?;
    publish_input.await??;
    Ok(output)
}

pub async fn run_listen_workflow() -> ExampleResult<()> {
    print_output(&execute_listen().await?)
}

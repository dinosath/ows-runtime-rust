//! Runs a small OWS workflow and prints the result.
//!
//! ```sh
//! cargo run -p ows-runtime --example run_workflow
//! ```

use ows_runtime::Runtime;
use serde_json::json;

const WORKFLOW: &str = r#"
document:
  dsl: '1.0.3'
  namespace: examples
  name: greet
  version: '0.1.0'
do:
  - buildGreeting:
      set:
        greeting: '${ "Hello, \(.name)!" }'
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Runtime::builder().build()?;
    let def = ows_runtime_dsl::from_yaml(WORKFLOW)?;
    let wf = runtime.register_definition(&def)?;
    let out = runtime.run(wf, json!({ "name": "World" })).await?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

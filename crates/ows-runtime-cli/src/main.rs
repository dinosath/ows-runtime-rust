//! The `ows-runtime` command line interface.
//!
//! This is a thin wrapper around the library. All logic lives in the
//! `ows-runtime` and `ows-runtime-dsl` crates (and the conformance module in
//! `ows_runtime_cli::conformance`).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ows-runtime",
    about = "Open Workflow Specification runtime",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validates a workflow definition.
    Validate {
        /// The workflow definition file (YAML or JSON).
        file: PathBuf,
    },
    /// Compiles a workflow definition to executable IR.
    Compile {
        /// The workflow definition file (YAML or JSON).
        file: PathBuf,
    },
    /// Runs a workflow definition to completion.
    Run {
        /// The workflow definition file (YAML or JSON).
        file: PathBuf,
        /// The workflow input as a YAML/JSON file.
        #[arg(long)]
        input: Option<PathBuf>,
    },
    /// Runs the OWS Conformance Test Kit against this runtime.
    Conformance {
        /// Path to the OWS specification repository (containing `ctk/features`).
        #[arg(long)]
        ctk: Option<PathBuf>,
        /// Output report file (JSON).
        #[arg(long)]
        output: Option<PathBuf>,
        /// Also run network-dependent scenarios (requires network access).
        #[arg(long)]
        include_network: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Validate { file } => {
            let def = ows_runtime_dsl::from_file(&file).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });
            let report = ows_runtime_dsl::validate(&def);
            if report.is_valid() {
                println!("valid");
            } else {
                for issue in &report.issues {
                    println!("{:?}", issue);
                }
                std::process::exit(1);
            }
        }
        Command::Compile { file } => {
            let def = ows_runtime_dsl::from_file(&file).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });
            let report = ows_runtime_dsl::validate(&def);
            if !report.is_valid() {
                for issue in &report.issues {
                    eprintln!("validation issue: {:?}", issue);
                }
                std::process::exit(1);
            }
            let runtime = ows_runtime::Runtime::builder()
                .build()
                .expect("runtime build");
            let compiled = runtime.compile(&def).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });
            let summary = serde_json::json!({
                "workflow": {
                    "namespace": compiled.id.namespace,
                    "name": compiled.id.name,
                    "version": compiled.id.version,
                },
                "tasks": compiled.tasks.len(),
                "task_types": compiled.tasks.iter().map(|t| t.type_name()).collect::<Vec<_>>(),
                "timeout_ms": compiled.timeout.map(|d| d.as_millis()),
                "schedule": compiled.schedule.is_some(),
            });
            println!("{}", serde_json::to_string_pretty(&summary).unwrap());
        }
        Command::Run { file, input } => {
            let def = ows_runtime_dsl::from_file(&file).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });
            let report = ows_runtime_dsl::validate(&def);
            if !report.is_valid() {
                for issue in &report.issues {
                    eprintln!("validation issue: {:?}", issue);
                }
                std::process::exit(1);
            }
            let runtime = ows_runtime::Runtime::builder()
                .build()
                .expect("runtime build");
            let wf = runtime.register_definition(&def).expect("compile");
            let input = match input {
                Some(path) => {
                    let text = std::fs::read_to_string(&path).expect("read input");
                    serde_yaml::from_str(&text).expect("parse input as YAML")
                }
                None => serde_json::Value::Null,
            };
            match runtime.run(wf, input).await {
                Ok(output) => {
                    println!("{}", serde_yaml::to_string(&output).unwrap());
                }
                Err(err) => {
                    eprintln!(
                        "workflow faulted: {}",
                        serde_json::to_string_pretty(&err.problem).unwrap()
                    );
                    std::process::exit(1);
                }
            }
        }
        Command::Conformance {
            ctk,
            output,
            include_network,
        } => {
            let ctk_dir = ctk.unwrap_or_else(|| {
                let repo =
                    std::env::var("OWS_SPEC_REPO").unwrap_or_else(|_| "specification".to_string());
                PathBuf::from(repo).join("ctk").join("features")
            });
            let report =
                ows_runtime_cli::conformance::run_ctk_with(&ctk_dir, include_network).await;
            if let Some(out) = output {
                let text = serde_json::to_string_pretty(&report).unwrap();
                std::fs::write(out, text).expect("write report");
            }
            print_human(&report);
            if report.has_failures() {
                std::process::exit(1);
            }
        }
    }
}

fn print_human(report: &ows_runtime_cli::conformance::ConformanceReport) {
    println!("OWS Conformance Report");
    println!("======================");
    for scenario in &report.scenarios {
        let status = match scenario.status {
            ows_runtime_cli::conformance::ScenarioStatus::Pass => "PASS",
            ows_runtime_cli::conformance::ScenarioStatus::Fail => "FAIL",
            ows_runtime_cli::conformance::ScenarioStatus::Skip => "SKIP",
        };
        println!("{status:4}  {}.{}", scenario.feature, scenario.name);
    }
    println!("----------------------");
    println!(
        "Total: {}  Pass: {}  Fail: {}  Skip: {}",
        report.total(),
        report.passes,
        report.failures,
        report.skips
    );
}

//! The `ows-runtime` command line interface.
//!
//! This is a thin wrapper around the library. All logic lives in the
//! `ows-runtime` and `ows-runtime-dsl` crates (and the conformance module in
//! `ows_runtime_cli::conformance`).

use std::io::{self, Read, Write};
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
    /// Runs a workflow definition to completion. If it declares input and
    /// --input is omitted, JSON/YAML input is read interactively from stdin.
    Run {
        /// The workflow definition file (YAML or JSON).
        file: PathBuf,
        /// The workflow input as a YAML/JSON file; omit it to be prompted when
        /// the workflow declares input.
        #[arg(long)]
        input: Option<PathBuf>,
        /// Novel URL to pass as `novel_url` workflow input.
        #[arg(long = "url", visible_alias = "novel-url", conflicts_with = "input")]
        novel_url: Option<String>,
        /// First chapter to pass as `chapter_from` workflow input.
        #[arg(
            long = "from",
            visible_alias = "chapter-from",
            conflicts_with = "input"
        )]
        chapter_from: Option<u64>,
        /// Last chapter to pass as `chapter_to` workflow input.
        #[arg(long = "to", visible_alias = "chapter-to", conflicts_with = "input")]
        chapter_to: Option<u64>,
        /// Save the final workflow output to this file instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Allow outbound network access (deny-by-default).
        #[arg(long)]
        allow_network: bool,
        /// Allow shell/script execution for `run` tasks (deny-by-default).
        #[arg(long)]
        allow_scripts: bool,
        /// Allow container execution for `run` tasks via Docker (deny-by-default).
        #[arg(long)]
        allow_containers: bool,
        /// A secret as `NAME=VALUE` (repeatable), exposed as `$secrets`.
        #[arg(long = "secret", value_name = "NAME=VALUE")]
        secrets: Vec<String>,
        /// A catalog as `ENDPOINT=PATH` (repeatable); the file is parsed and
        /// served for the workflow's `use.catalogs` endpoint.
        #[arg(long = "catalog", value_name = "ENDPOINT=PATH")]
        catalogs: Vec<String>,
        /// Enable the filesystem catalog resolver (for `file://` endpoints).
        #[arg(long)]
        catalog_files: bool,
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
        Command::Run {
            file,
            input,
            novel_url,
            chapter_from,
            chapter_to,
            output: output_path,
            allow_network,
            allow_scripts,
            allow_containers,
            secrets,
            catalogs,
            catalog_files,
        } => {
            init_run_logging();
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

            let policy = ows_runtime_core::RuntimePolicy {
                allow_network,
                ..Default::default()
            };
            let mut builder = ows_runtime::Runtime::builder().with_policy(policy);

            if allow_scripts || allow_containers {
                let mut runner = ows_runtime::TokioProcessRunner::new();
                if allow_scripts {
                    runner = runner.allow_scripts();
                }
                if allow_containers {
                    runner = runner.allow_containers();
                }
                builder = builder.with_process(std::sync::Arc::new(runner));
            }

            let mut secret_map = std::collections::HashMap::new();
            for spec in &secrets {
                match spec.split_once('=') {
                    Some((name, value)) => {
                        secret_map.insert(name.to_string(), value.to_string());
                    }
                    None => {
                        eprintln!("error: --secret must be NAME=VALUE, got `{spec}`");
                        std::process::exit(1);
                    }
                }
            }
            if !secret_map.is_empty() {
                builder = builder.with_secrets(secret_map);
            }

            if catalog_files && catalogs.is_empty() {
                builder = builder.with_catalog_resolver(std::sync::Arc::new(
                    ows_runtime::catalog::FileCatalogResolver,
                ));
            } else if !catalogs.is_empty() {
                let mut resolver = ows_runtime::catalog::StaticCatalogResolver::new();
                for spec in &catalogs {
                    let Some((endpoint, path)) = spec.split_once('=') else {
                        eprintln!("error: --catalog must be ENDPOINT=PATH, got `{spec}`");
                        std::process::exit(1);
                    };
                    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
                        eprintln!("error: read catalog `{path}`: {e}");
                        std::process::exit(1);
                    });
                    let collection = ows_runtime::catalog::parse_catalog_document(&text)
                        .unwrap_or_else(|e| {
                            eprintln!("error: parse catalog `{path}`: {e}");
                            std::process::exit(1);
                        });
                    resolver.insert(endpoint, collection);
                }
                builder = builder.with_catalog_resolver(std::sync::Arc::new(resolver));
            }

            let runtime = builder.build().expect("runtime build");
            let wf = runtime
                .register_definition_resolved(&def)
                .await
                .expect("compile");
            if (chapter_from.is_some() || chapter_to.is_some()) && novel_url.is_none() {
                eprintln!("error: --from and --to require --url");
                std::process::exit(1);
            }
            let cli_input = novel_url.map(|url| {
                let mut value = serde_json::Map::new();
                value.insert("novel_url".to_string(), serde_json::Value::String(url));
                if let Some(chapter_from) = chapter_from {
                    value.insert("chapter_from".to_string(), serde_json::json!(chapter_from));
                }
                if let Some(chapter_to) = chapter_to {
                    value.insert("chapter_to".to_string(), serde_json::json!(chapter_to));
                }
                serde_json::Value::Object(value)
            });
            let input = match (input, cli_input) {
                (Some(_), Some(_)) => {
                    eprintln!("error: --input cannot be combined with --url, --from, or --to");
                    std::process::exit(1);
                }
                (Some(path), None) => {
                    let text = std::fs::read_to_string(&path).expect("read input");
                    serde_yaml::from_str(&text).expect("parse input as YAML")
                }
                (None, Some(value)) => value,
                (None, None) if def.input.is_some() => prompt_for_input(&def),
                (None, None) => serde_json::Value::Null,
            };
            match runtime.run(wf, input).await {
                Ok(result) => {
                    let rendered = serde_yaml::to_string(&result).unwrap_or_else(|error| {
                        eprintln!("error: serialize workflow output: {error}");
                        std::process::exit(1);
                    });
                    if let Some(path) = output_path {
                        std::fs::write(&path, rendered).unwrap_or_else(|error| {
                            eprintln!("error: write workflow output `{}`: {error}", path.display());
                            std::process::exit(1);
                        });
                        eprintln!("workflow output written to {}", path.display());
                    } else {
                        print!("{rendered}");
                    }
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

fn init_run_logging() {
    let filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("ows_runtime=debug".parse().expect("valid log directive"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(true)
        .try_init();
}

/// Prompts for a workflow's declared input when `run` is used interactively.
///
/// JSON is valid YAML, so accepting YAML here also accepts structured JSON.
/// Reading until EOF keeps the prompt useful for nested objects and arrays;
/// finish interactive input with Ctrl-D.
fn prompt_for_input(
    def: &ows_runtime_dsl::models::workflow::WorkflowDefinition,
) -> serde_json::Value {
    eprintln!("This workflow declares input.");
    if let Some(schema) = def.input.as_ref().and_then(|input| input.schema.as_ref()) {
        if let Ok(schema) = serde_yaml::to_string(schema) {
            eprintln!("Input schema:\n{schema}");
        }
    }
    eprintln!("Enter workflow input as JSON or YAML, then press Ctrl-D:");
    io::stderr().flush().expect("flush input prompt");

    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .unwrap_or_else(|error| {
            eprintln!("error: read workflow input: {error}");
            std::process::exit(1);
        });
    if text.trim().is_empty() {
        eprintln!("error: workflow input cannot be empty");
        std::process::exit(1);
    }
    serde_yaml::from_str(&text).unwrap_or_else(|error| {
        eprintln!("error: parse workflow input as JSON/YAML: {error}");
        std::process::exit(1);
    })
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

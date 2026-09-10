//! Integration tests for the native gRPC transport wiring (`call: grpc`).
//!
//! The full dynamic call path (path, framing, protobuf encode/decode) is
//! exercised against an in-memory service in the `grpc` module's unit tests.
//! These tests verify the runtime wiring: argument validation, descriptor
//! resolution and network policy enforcement.
#![cfg(feature = "grpc-native")]

use ows_runtime::Runtime;
use ows_runtime_core::{ErrorKind, RuntimePolicy};
use serde_json::Value;

const PROTO: &str = r#"
syntax = "proto3";
package greet;
message HelloRequest { string name = 1; }
message HelloReply { string message = 1; }
service Greeter { rpc SayHello (HelloRequest) returns (HelloReply); }
"#;

fn workflow(proto_yaml: &str, host: &str, port: u16, service: &str, method: &str) -> String {
    format!(
        r#"
document: {{ dsl: '1.0.3', namespace: grpc, name: native, version: '1.0.0' }}
do:
  - call:
      call: grpc
      with:
        proto:
{proto_yaml}
        service:
          name: {service}
          host: {host}
          port: {port}
        method: {method}
        arguments:
          name: Ada
"#
    )
}

fn inline_proto() -> String {
    let mut out = String::from("          content: |\n");
    for line in PROTO.lines() {
        out.push_str("            ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn network_policy() -> RuntimePolicy {
    RuntimePolicy {
        allow_network: true,
        allowed_hosts: vec!["127.0.0.1".into(), "localhost".into()],
        allowed_schemes: vec!["http".into()],
        ..Default::default()
    }
}

#[tokio::test]
async fn native_grpc_resolves_service_before_connecting() {
    let runtime = Runtime::builder().build().unwrap();
    // Unknown service: fails during descriptor resolution (no network).
    let yaml = workflow(
        &inline_proto(),
        "localhost",
        50051,
        "Nope.Missing",
        "SayHello",
    );
    let def = ows_runtime_dsl::from_yaml(&yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Semantic);
    assert!(err.problem.detail.unwrap_or_default().contains("not found"));
}

#[tokio::test]
async fn native_grpc_invalid_proto_is_semantic_error() {
    let runtime = Runtime::builder().build().unwrap();
    let bad_proto = "          content: |\n            this is not a proto\n";
    let yaml = workflow(bad_proto, "localhost", 50051, "greet.Greeter", "SayHello");
    let def = ows_runtime_dsl::from_yaml(&yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Semantic);
}

#[tokio::test]
async fn native_grpc_is_denied_by_default_policy() {
    let runtime = Runtime::builder().build().unwrap();
    let yaml = workflow(
        &inline_proto(),
        "example.com",
        443,
        "greet.Greeter",
        "SayHello",
    );
    let def = ows_runtime_dsl::from_yaml(&yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Policy);
}

#[tokio::test]
async fn native_grpc_reports_connection_failure() {
    // A port that is bound and immediately released is (almost certainly) closed.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let runtime = Runtime::builder()
        .with_policy(network_policy())
        .build()
        .unwrap();
    let yaml = workflow(
        &inline_proto(),
        "127.0.0.1",
        port,
        "greet.Greeter",
        "SayHello",
    );
    let def = ows_runtime_dsl::from_yaml(&yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Communication);
}

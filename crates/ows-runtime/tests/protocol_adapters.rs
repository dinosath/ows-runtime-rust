//! Offline tests for the gRPC / AsyncAPI / A2A / MCP `call` function adapters.
//!
//! Each adapter is exercised against a minimal in-process HTTP server so the
//! tests are deterministic and require no external network or live services.
#![cfg(feature = "http")]

use ows_runtime::Runtime;
use ows_runtime_core::RuntimePolicy;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves one canned JSON response per accepted connection.
async fn serve_once(addr: std::net::SocketAddr, body: &'static str) {
    let listener = TcpListener::bind(addr).await.unwrap();
    for _ in 0..4 {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 8192];
        let _ = socket.read(&mut buf).await;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(resp.as_bytes()).await;
        let _ = socket.shutdown().await;
    }
}

fn free_addr() -> (std::net::SocketAddr, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);
    (addr, port)
}

fn network_policy() -> RuntimePolicy {
    RuntimePolicy {
        allow_network: true,
        ..Default::default()
    }
}

async fn run_call(yaml: &str) -> Value {
    let runtime = Runtime::builder()
        .with_policy(network_policy())
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(yaml).unwrap();
    let wf = runtime.register_definition(&def).unwrap();
    runtime.run(wf, Value::Null).await.unwrap()
}

#[tokio::test]
async fn mcp_call_invokes_tool() {
    let (addr, port) = free_addr();
    let body = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"hello mcp"}],"isError":false}}"#;
    tokio::spawn(serve_once(addr, body));

    let yaml = format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: mcp-call
  version: '1.0.0'
do:
  - useMcp:
      call: mcp
      with:
        endpoint: http://127.0.0.1:{port}
        tool: greet
        arguments: {{ name: world }}
"#
    );
    let out = run_call(&yaml).await;
    assert_eq!(out, json!("hello mcp"));
}

#[tokio::test]
async fn a2a_call_sends_message() {
    let (addr, port) = free_addr();
    let body = r#"{"jsonrpc":"2.0","id":1,"result":{"kind":"text","text":"hello a2a"}}"#;
    tokio::spawn(serve_once(addr, body));

    let yaml = format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: a2a-call
  version: '1.0.0'
do:
  - send:
      call: a2a
      with:
        endpoint: http://127.0.0.1:{port}
        message:
          text: hi
"#
    );
    let out = run_call(&yaml).await;
    assert_eq!(out, json!({ "kind": "text", "text": "hello a2a" }));
}

#[tokio::test]
async fn grpc_call_json_transcoding() {
    let (addr, port) = free_addr();
    let body = r#"{"message":"hi from grpc"}"#;
    tokio::spawn(serve_once(addr, body));

    let yaml = format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: grpc-call
  version: '1.0.0'
do:
  - say:
      call: grpc
      with:
        endpoint: http://127.0.0.1:{port}
        message:
          name: bob
"#
    );
    let out = run_call(&yaml).await;
    assert_eq!(out, json!({ "message": "hi from grpc" }));
}

#[tokio::test]
async fn asyncapi_call_publishes_message() {
    let (addr, port) = free_addr();
    let body = r#"{"ack":true}"#;
    tokio::spawn(serve_once(addr, body));

    // An inline AsyncAPI document with a single publishable channel. The
    // `endpoint` override points at the loopback server.
    let doc = json!({
        "asyncapi": "3.0.0",
        "channels": {
            "user/signedup": {
                "publish": { "operationId": "onUserSignedUp" }
            }
        }
    });
    let yaml = format!(
        r#"
document:
  dsl: '1.0.3'
  namespace: default
  name: asyncapi-call
  version: '1.0.0'
do:
  - publish:
      call: asyncapi
      with:
        document: {document}
        channel: user/signedup
        endpoint: http://127.0.0.1:{port}
        message: {{ user: "ann" }}
"#,
        document = doc,
    );
    let out = run_call(&yaml).await;
    assert_eq!(out, json!({ "ack": true }));
}

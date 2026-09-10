//! HTTP catalog resolver integration boundary tests (loopback server + policy).
#![cfg(feature = "http")]

use std::sync::Arc;

use ows_runtime::catalog::{CatalogResolver, HttpCatalogResolver};
use ows_runtime::Runtime;
use ows_runtime_core::{ErrorKind, RuntimePolicy};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CATALOG: &str = r#"{"errors":{"catalogError":{"type":"https://example.com/errors/catalog","title":"Catalog Error","status":418}}}"#;

async fn serve_json(body: &'static str) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    addr
}

fn workflow(endpoint: &str) -> String {
    format!(
        r#"
document: {{ dsl: '1.0.3', namespace: net, name: http-catalog, version: '1.0.0' }}
use:
  catalogs:
    errors:
      endpoint: {endpoint}
do:
  - r:
      raise:
        error: catalogError
"#
    )
}

fn allow_localhost() -> RuntimePolicy {
    RuntimePolicy {
        allow_network: true,
        allowed_hosts: vec!["127.0.0.1".into()],
        allowed_schemes: vec!["http".into()],
        ..Default::default()
    }
}

#[tokio::test]
async fn http_catalog_resolver_fetches_components() {
    let addr = serve_json(CATALOG).await;
    let resolver = HttpCatalogResolver::new();
    let collection = resolver
        .resolve(&format!("http://{addr}/catalog.json"))
        .await
        .unwrap();
    assert!(collection.errors.unwrap().contains_key("catalogError"));
}

#[tokio::test]
async fn runtime_resolves_an_http_catalog_end_to_end() {
    let addr = serve_json(CATALOG).await;
    let runtime = Runtime::builder()
        .with_policy(allow_localhost())
        .with_catalog_resolver(Arc::new(HttpCatalogResolver::new()))
        .build()
        .unwrap();

    let def =
        ows_runtime_dsl::from_yaml(&workflow(&format!("http://{addr}/catalog.json"))).unwrap();
    let wf = runtime.register_definition_resolved(&def).await.unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.problem.type_, "https://example.com/errors/catalog");
    assert_eq!(err.problem.status, 418);
}

#[tokio::test]
async fn http_catalog_is_denied_by_default_policy() {
    let runtime = Runtime::builder()
        .with_catalog_resolver(Arc::new(HttpCatalogResolver::new()))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(&workflow("https://catalog.example/errors")).unwrap();
    let err = runtime.resolve_definition(&def).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Policy);
}

#[tokio::test]
async fn http_catalog_fetch_failure_surfaces() {
    // Bind then release a port so the fetch fails to connect.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let runtime = Runtime::builder()
        .with_policy(allow_localhost())
        .with_catalog_resolver(Arc::new(HttpCatalogResolver::new()))
        .build()
        .unwrap();
    let def =
        ows_runtime_dsl::from_yaml(&workflow(&format!("http://127.0.0.1:{port}/catalog.json")))
            .unwrap();
    let err = runtime.resolve_definition(&def).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Communication);
}

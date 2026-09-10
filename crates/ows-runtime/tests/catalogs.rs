//! Integration tests for `use.catalogs` resolution.

use std::sync::Arc;

use ows_runtime::catalog::{CatalogResolver, StaticCatalogResolver};
use ows_runtime::dsl_models::{ComponentDefinitionCollection, ErrorDefinition};
use ows_runtime::Runtime;
use serde_json::{json, Value};

fn error_catalog() -> ComponentDefinitionCollection {
    let mut collection = ComponentDefinitionCollection::default();
    collection.errors = Some(
        [(
            "catalogError".to_string(),
            ErrorDefinition::new("https://example.com/errors/catalog", "Catalog Error", json!(418), None, None),
        )]
        .into_iter()
        .collect(),
    );
    collection
}

#[tokio::test]
async fn resolves_catalog_error_reference() {
    let resolver = StaticCatalogResolver::new().with("https://catalog.example/errors", error_catalog());
    let runtime = Runtime::builder()
        .with_catalog_resolver(Arc::new(resolver))
        .build()
        .unwrap();

    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: catalog, version: '1.0.0' }
use:
  catalogs:
    errors:
      endpoint: https://catalog.example/errors
do:
  - r:
      raise:
        error: catalogError
"#,
    )
    .unwrap();

    let resolved = runtime.resolve_definition(&def).await.unwrap();
    assert!(resolved.use_.as_ref().unwrap().errors.as_ref().unwrap().contains_key("catalogError"));

    let wf = runtime.register_definition_resolved(&def).await.unwrap();
    let err = runtime.run(wf, Value::Null).await.unwrap_err();
    assert_eq!(err.problem.type_, "https://example.com/errors/catalog");
    assert_eq!(err.problem.status, 418);
}

#[tokio::test]
async fn catalogs_are_ignored_without_a_resolver() {
    let runtime = Runtime::builder().build().unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: catalog-none, version: '1.0.0' }
use:
  catalogs:
    errors:
      endpoint: https://catalog.example/errors
do:
  - r:
      raise:
        error: catalogError
"#,
    )
    .unwrap();
    // Unchanged without a resolver.
    let resolved = runtime.resolve_definition(&def).await.unwrap();
    assert!(resolved.use_.as_ref().unwrap().errors.is_none());
    // Compiles, but the undefined error faults at runtime.
    let wf = runtime.register_definition_resolved(&def).await.unwrap();
    assert!(runtime.run(wf, Value::Null).await.is_err());
}

#[tokio::test]
async fn resolves_nested_catalogs() {
    let mut inner = ComponentDefinitionCollection::default();
    inner.secrets = Some(vec!["fromNested".to_string()]);

    let mut outer = ComponentDefinitionCollection::default();
    outer.catalogs = Some(
        [(
            "nested".to_string(),
            ows_runtime::dsl_models::CatalogDefinition {
                endpoint: ows_runtime::dsl_models::OneOfEndpointDefinitionOrUri::Uri(
                    "https://catalog.example/nested".to_string(),
                ),
            },
        )]
        .into_iter()
        .collect(),
    );

    let resolver = StaticCatalogResolver::new()
        .with("https://catalog.example/outer", outer)
        .with("https://catalog.example/nested", inner);
    let runtime = Runtime::builder()
        .with_catalog_resolver(Arc::new(resolver))
        .build()
        .unwrap();

    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: catalog-nested, version: '1.0.0' }
use:
  catalogs:
    outer:
      endpoint: https://catalog.example/outer
do:
  - s: { set: { x: 1 } }
"#,
    )
    .unwrap();
    let resolved = runtime.resolve_definition(&def).await.unwrap();
    assert_eq!(
        resolved.use_.as_ref().unwrap().secrets.as_ref().unwrap(),
        &vec!["fromNested".to_string()]
    );
}

/// A resolver that always fails, to prove resolution errors surface.
struct FailingResolver;

#[async_trait::async_trait]
impl CatalogResolver for FailingResolver {
    async fn resolve(
        &self,
        _endpoint: &str,
    ) -> Result<ComponentDefinitionCollection, ows_runtime_core::WorkflowError> {
        Err(ows_runtime_core::WorkflowError::standard(
            ows_runtime_core::ErrorKind::Communication,
            ows_runtime_core::StandardErrorType::Communication,
        ))
    }
}

#[tokio::test]
async fn catalog_resolution_errors_surface() {
    let runtime = Runtime::builder()
        .with_catalog_resolver(Arc::new(FailingResolver))
        .build()
        .unwrap();
    let def = ows_runtime_dsl::from_yaml(
        r#"
document: { dsl: '1.0.3', namespace: default, name: catalog-fail, version: '1.0.0' }
use:
  catalogs:
    errors:
      endpoint: https://catalog.example/down
do:
  - s: { set: { x: 1 } }
"#,
    )
    .unwrap();
    assert!(runtime.resolve_definition(&def).await.is_err());
}

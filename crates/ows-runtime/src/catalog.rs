//! Catalog resolution for `use.catalogs`.
//!
//! OWS workflows may import reusable components (functions, errors, retries,
//! timeouts, authentications, ...) from external catalogs. The runtime keeps
//! the transport behind the [`CatalogResolver`] trait so catalogs can be served
//! from HTTP, the filesystem, or embedded data, without the engine knowing.

use std::collections::HashMap;

use serde_json::Value;

use crate::dsl_models::ComponentDefinitionCollection;
use ows_runtime_core::{ErrorKind, ProblemDetails, StandardErrorType, WorkflowError};

/// Resolves a catalog endpoint into a collection of reusable components.
#[async_trait::async_trait]
pub trait CatalogResolver: Send + Sync {
    /// Fetches and decodes the catalog at `endpoint`.
    async fn resolve(&self, endpoint: &str)
        -> Result<ComponentDefinitionCollection, WorkflowError>;
}

fn catalog_error(detail: String) -> WorkflowError {
    let mut err = WorkflowError::new(
        ErrorKind::Communication,
        ProblemDetails::standard(StandardErrorType::Communication),
    );
    err.problem.detail = Some(detail);
    err
}

fn schema_error(detail: String) -> WorkflowError {
    let mut err = WorkflowError::new(
        ErrorKind::Schema,
        ProblemDetails::standard(StandardErrorType::Validation),
    );
    err.problem.detail = Some(detail);
    err
}

/// Parses a catalog document, accepting either a bare component collection or a
/// wrapper object exposing it under `use`/`components`.
pub fn parse_catalog_document(text: &str) -> Result<ComponentDefinitionCollection, WorkflowError> {
    let value: Value = serde_yaml::from_str(text)
        .or_else(|_| serde_json::from_str(text))
        .map_err(|e| schema_error(format!("invalid catalog document: {e}")))?;
    let collection = match &value {
        Value::Object(map) if map.contains_key("use") => map.get("use").cloned(),
        Value::Object(map) if map.contains_key("components") => map.get("components").cloned(),
        _ => Some(value),
    }
    .unwrap_or(Value::Null);
    serde_json::from_value(collection)
        .map_err(|e| schema_error(format!("invalid catalog components: {e}")))
}

/// A catalog resolver backed by an in-memory endpoint → collection map.
#[derive(Debug, Clone, Default)]
pub struct StaticCatalogResolver {
    catalogs: HashMap<String, ComponentDefinitionCollection>,
}

impl StaticCatalogResolver {
    /// Creates an empty resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a catalog for an endpoint.
    pub fn insert(&mut self, endpoint: impl Into<String>, catalog: ComponentDefinitionCollection) {
        self.catalogs.insert(endpoint.into(), catalog);
    }

    /// Registers a catalog and returns the resolver (builder style).
    pub fn with(
        mut self,
        endpoint: impl Into<String>,
        catalog: ComponentDefinitionCollection,
    ) -> Self {
        self.insert(endpoint, catalog);
        self
    }
}

#[async_trait::async_trait]
impl CatalogResolver for StaticCatalogResolver {
    async fn resolve(
        &self,
        endpoint: &str,
    ) -> Result<ComponentDefinitionCollection, WorkflowError> {
        self.catalogs
            .get(endpoint)
            .cloned()
            .ok_or_else(|| catalog_error(format!("no static catalog registered for `{endpoint}`")))
    }
}

/// A catalog resolver that reads `file://` endpoints (or bare paths).
#[derive(Debug, Clone, Default)]
pub struct FileCatalogResolver;

#[async_trait::async_trait]
impl CatalogResolver for FileCatalogResolver {
    async fn resolve(
        &self,
        endpoint: &str,
    ) -> Result<ComponentDefinitionCollection, WorkflowError> {
        let path = endpoint.strip_prefix("file://").unwrap_or(endpoint);
        let text = std::fs::read_to_string(path)
            .map_err(|e| catalog_error(format!("failed to read catalog `{endpoint}`: {e}")))?;
        parse_catalog_document(&text)
    }
}

/// A catalog resolver that fetches catalogs over HTTP(S) as JSON or YAML.
#[cfg(feature = "http")]
#[derive(Clone)]
pub struct HttpCatalogResolver {
    client: reqwest::Client,
}

#[cfg(feature = "http")]
impl HttpCatalogResolver {
    /// Creates a new HTTP catalog resolver.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder().build().unwrap_or_default(),
        }
    }
}

#[cfg(feature = "http")]
impl Default for HttpCatalogResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "http")]
#[async_trait::async_trait]
impl CatalogResolver for HttpCatalogResolver {
    async fn resolve(
        &self,
        endpoint: &str,
    ) -> Result<ComponentDefinitionCollection, WorkflowError> {
        let response = self
            .client
            .get(endpoint)
            .send()
            .await
            .map_err(|e| catalog_error(format!("failed to fetch catalog `{endpoint}`: {e}")))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(catalog_error(format!(
                "catalog `{endpoint}` returned status {status}"
            )));
        }
        let text = response
            .text()
            .await
            .map_err(|e| catalog_error(format!("failed to read catalog `{endpoint}`: {e}")))?;
        parse_catalog_document(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_and_wrapped_documents() {
        let bare = r#"
errors:
  myErr:
    type: https://x/e
    title: E
    status: 400
"#;
        let c = parse_catalog_document(bare).unwrap();
        assert!(c.errors.unwrap().contains_key("myErr"));

        let wrapped = r#"{"use": {"secrets": ["a", "b"]}}"#;
        let c = parse_catalog_document(wrapped).unwrap();
        assert_eq!(c.secrets.unwrap(), vec!["a".to_string(), "b".to_string()]);

        let components = r#"{"components": {"secrets": ["x"]}}"#;
        let c = parse_catalog_document(components).unwrap();
        assert_eq!(c.secrets.unwrap(), vec!["x".to_string()]);
    }

    #[test]
    fn invalid_document_is_schema_error() {
        let err = parse_catalog_document("secrets: not-a-list").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Schema);
    }

    #[tokio::test]
    async fn static_resolver_resolves_known_endpoint() {
        let mut r = StaticCatalogResolver::new();
        let c = ComponentDefinitionCollection {
            secrets: Some(vec!["s".into()]),
            ..Default::default()
        };
        r.insert("https://cat", c);
        assert!(r.resolve("https://cat").await.is_ok());
        assert!(r.resolve("https://missing").await.is_err());
    }

    #[tokio::test]
    async fn file_resolver_reads_catalog() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("ows-catalog-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"errors":{"e":{"type":"t","title":"T","status":400}}}"#,
        )
        .unwrap();
        let r = FileCatalogResolver;
        let c = r.resolve(path.to_str().unwrap()).await.unwrap();
        assert!(c.errors.unwrap().contains_key("e"));
        let _ = std::fs::remove_file(&path);
    }
}

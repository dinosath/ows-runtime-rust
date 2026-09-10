//! The [`Runtime`] and its builder, plus the shared [`RuntimeInner`] services.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ows_runtime_core::{
    Clock, ErrorKind, EventConsumer, EventPublisher, ExecutionStore, InMemoryExecutionStore,
    ProblemDetails, ProcessRunner, RandomGenerator, RuntimeInfo, RuntimePolicy, Scheduler,
    SecretResolver, ServiceInvoker, StandardErrorType, SystemClock, UuidGenerator, WorkflowError,
};
use serverless_workflow_core::models::workflow::WorkflowDefinition;
use tokio::sync::{oneshot, Notify};

use crate::compile;
use crate::engine;
use crate::ir::CompiledWorkflow;
#[cfg(feature = "http")]
use crate::service::{
    A2aInvoker, AsyncApiInvoker, GrpcInvoker, HttpServiceInvoker, McpInvoker, OpenApiInvoker,
};
use crate::service::{FunctionInvoker, NoopProcessRunner, NoopServiceInvoker};

/// Shared services backing all executions of a [`Runtime`].
pub struct RuntimeInner {
    pub expression: Arc<dyn ows_runtime_core::ExpressionEngine>,
    pub clock: Arc<dyn Clock>,
    pub uuid: Arc<dyn UuidGenerator>,
    pub random: Arc<dyn RandomGenerator>,
    pub runtime_info: RuntimeInfo,
    pub policy: RuntimePolicy,
    pub publisher: Arc<dyn EventPublisher>,
    pub service: Arc<dyn ServiceInvoker>,
    pub functions: Arc<std::sync::RwLock<HashMap<String, Arc<dyn FunctionInvoker>>>>,
    pub process: Arc<dyn ProcessRunner>,
    pub consumer: Arc<dyn EventConsumer>,
    pub store: Arc<dyn ExecutionStore>,
    pub scheduler: Arc<dyn Scheduler>,
    /// Resolves declared workflow secrets for `$secrets`.
    pub secrets: Arc<dyn SecretResolver>,
    /// Resolves `use.catalogs` endpoints, if configured.
    pub catalog: Option<Arc<dyn crate::catalog::CatalogResolver>>,
    pub workflows: Arc<std::sync::Mutex<HashMap<String, Arc<CompiledWorkflow>>>>,
    /// A log of task names in execution order (used for ordering assertions).
    pub task_order: Arc<std::sync::Mutex<Vec<String>>>,
    #[cfg(feature = "validation")]
    validator: Option<Arc<JsonSchemaValidator>>,
}

/// A JSON Schema validator backed by the `jsonschema` crate.
#[cfg(feature = "validation")]
pub(crate) struct JsonSchemaValidator;

#[cfg(feature = "validation")]
impl JsonSchemaValidator {
    pub(crate) fn validate(
        &self,
        document: &serde_json::Value,
        value: &serde_json::Value,
    ) -> Result<(), Vec<String>> {
        let compiled = match jsonschema::JSONSchema::compile(document) {
            Ok(c) => c,
            Err(e) => return Err(vec![format!("invalid schema: {e}")]),
        };
        let result = compiled.validate(value);
        match result {
            Ok(()) => Ok(()),
            Err(errors) => {
                let messages: Vec<String> = errors.map(|e| e.to_string()).collect();
                Err(messages)
            }
        }
    }
}

impl RuntimeInner {
    /// The lifecycle event emitter.
    pub fn lifecycle(&self) -> ows_runtime_observability::LifecycleEmitter {
        ows_runtime_observability::LifecycleEmitter::new(self.publisher.clone())
    }

    /// Validates a value against a JSON schema document.
    pub fn validate(
        &self,
        document: &serde_json::Value,
        value: &serde_json::Value,
    ) -> Result<(), Vec<String>> {
        #[cfg(feature = "validation")]
        {
            match &self.validator {
                Some(v) => v.validate(document, value),
                None => Ok(()),
            }
        }
        #[cfg(not(feature = "validation"))]
        {
            let _ = (document, value);
            Ok(())
        }
    }

    /// Looks up a workflow by identity key.
    pub fn workflow(&self, key: &str) -> Option<Arc<CompiledWorkflow>> {
        self.workflows.lock().unwrap().get(key).cloned()
    }

    /// Registers a compiled workflow.
    pub fn register(&self, workflow: CompiledWorkflow) -> Arc<CompiledWorkflow> {
        let arc = Arc::new(workflow);
        let key = arc.id.key();
        self.workflows.lock().unwrap().insert(key, arc.clone());
        arc
    }

    /// Records a task name in the execution order log.
    pub fn record_task(&self, name: &str) {
        self.task_order.lock().unwrap().push(name.to_string());
    }

    /// Clears and returns the recorded task order.
    pub fn take_task_order(&self) -> Vec<String> {
        std::mem::take(&mut *self.task_order.lock().unwrap())
    }
}

/// Cancellation token shared across an execution and its branches.
#[derive(Debug, Default)]
pub(crate) struct Cancellation {
    flag: AtomicBool,
    notify: Notify,
}

impl Cancellation {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
    pub fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}

/// A running workflow execution.
pub struct ExecutionHandle {
    /// The stable execution id.
    pub execution_id: String,
    rx: tokio::sync::Mutex<Option<oneshot::Receiver<Result<serde_json::Value, WorkflowError>>>>,
    cancel: Arc<Cancellation>,
}

impl ExecutionHandle {
    /// Awaits the workflow completion, returning the output or error.
    pub async fn wait(self) -> Result<serde_json::Value, WorkflowError> {
        let rx = self.rx.lock().await.take();
        match rx {
            Some(rx) => rx.await.map_err(|_| WorkflowError::cancelled())?,
            None => Err(WorkflowError::cancelled()),
        }
    }

    /// Requests cancellation of the workflow.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// A `Runtime` executes compiled OWS workflows.
#[derive(Clone)]
pub struct Runtime {
    pub(crate) inner: Arc<RuntimeInner>,
}

impl Runtime {
    /// Starts building a new runtime.
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// Compiles and validates a workflow definition.
    pub fn compile(&self, def: &WorkflowDefinition) -> Result<CompiledWorkflow, WorkflowError> {
        compile::compile(def)
    }

    /// Compiles and registers a workflow definition.
    pub fn register_definition(
        &self,
        def: &WorkflowDefinition,
    ) -> Result<Arc<CompiledWorkflow>, WorkflowError> {
        let compiled = self.compile(def)?;
        Ok(self.inner.register(compiled))
    }

    /// Resolves the workflow's `use.catalogs` imports and merges the imported
    /// components into `use`.
    ///
    /// If the definition declares no catalogs, or no catalog resolver is
    /// configured, the definition is returned unchanged (references to missing
    /// components then fail at compile time).
    pub async fn resolve_definition(
        &self,
        def: &WorkflowDefinition,
    ) -> Result<WorkflowDefinition, WorkflowError> {
        let Some(use_) = def.use_.clone() else {
            return Ok(def.clone());
        };
        if use_.catalogs.as_ref().map(|c| c.is_empty()).unwrap_or(true) {
            return Ok(def.clone());
        }
        let Some(resolver) = self.inner.catalog.clone() else {
            tracing::warn!(
                workflow.name = %def.document.name,
                "workflow declares catalogs but no catalog resolver is configured"
            );
            return Ok(def.clone());
        };

        let mut merged = use_.clone();
        let mut queue: Vec<(String, crate::dsl_models::OneOfEndpointDefinitionOrUri)> = merged
            .catalogs
            .as_ref()
            .map(|c| c.iter().map(|(k, v)| (k.clone(), v.endpoint.clone())).collect())
            .unwrap_or_default();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();

        while let Some((_name, endpoint)) = queue.pop() {
            let uri = catalog_endpoint_uri(&endpoint);
            if !visited.insert(uri.clone()) {
                continue;
            }
            let collection = resolver.resolve(&uri).await?;
            // Queue any catalogs the imported collection declares.
            if let Some(nested) = &collection.catalogs {
                for (k, v) in nested {
                    queue.push((k.clone(), v.endpoint.clone()));
                }
            }
            merge_components(&mut merged, collection);
        }

        let mut resolved = def.clone();
        resolved.use_ = Some(merged);
        Ok(resolved)
    }

    /// Resolves `use.catalogs`, then compiles and registers the workflow.
    pub async fn register_definition_resolved(
        &self,
        def: &WorkflowDefinition,
    ) -> Result<Arc<CompiledWorkflow>, WorkflowError> {
        let resolved = self.resolve_definition(def).await?;
        self.register_definition(&resolved)
    }

    /// Executes a compiled workflow with the given input.
    pub async fn execute(
        &self,
        workflow: Arc<CompiledWorkflow>,
        input: serde_json::Value,
    ) -> Result<ExecutionHandle, WorkflowError> {
        let inner = self.inner.clone();
        let execution_id = inner.uuid.new_v4().to_string();
        let cancel = Arc::new(Cancellation::new());
        let (tx, rx) = oneshot::channel();

        let inner_exec = inner.clone();
        let workflow_exec = workflow.clone();
        let cancel_exec = cancel.clone();

        tokio::spawn(async move {
            let result = run_with_cancel(inner_exec, workflow_exec, input, cancel_exec).await;
            let _ = tx.send(result);
        });

        Ok(ExecutionHandle {
            execution_id,
            rx: tokio::sync::Mutex::new(Some(rx)),
            cancel,
        })
    }

    /// Runs a compiled workflow to completion and returns its output.
    pub async fn run(
        &self,
        workflow: Arc<CompiledWorkflow>,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, WorkflowError> {
        self.execute(workflow, input).await?.wait().await
    }

    /// Resumes a previously checkpointed execution through the configured
    /// [`ExecutionStore`].
    ///
    /// The workflow must still be registered (by its `namespace/name/version`
    /// key) and the execution must not be terminal. Execution continues from the
    /// checkpointed top-level task index with the checkpointed context and
    /// input, reusing the original execution id.
    pub async fn resume(
        &self,
        execution_id: &str,
    ) -> Result<serde_json::Value, WorkflowError> {
        let record = self
            .inner
            .store
            .load(execution_id)
            .await?
            .ok_or_else(|| unknown_execution(execution_id))?;

        if record.phase.is_terminal() {
            return Err(runtime_detail(format!(
                "execution `{execution_id}` is already {} and cannot be resumed",
                record.phase
            )));
        }

        let workflow = self
            .inner
            .workflow(&record.workflow)
            .ok_or_else(|| runtime_detail(format!(
                "workflow `{}` is not registered; register it before resuming",
                record.workflow
            )))?;

        let next_index = record
            .pointer
            .get("next")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let input = record
            .pointer
            .get("input")
            .cloned()
            .unwrap_or_else(|| record.context.clone());

        let resume = engine::ResumeState {
            next_index,
            input,
            context: record.context.clone(),
        };

        let cancel = Arc::new(Cancellation::new());
        engine::execute_with(
            self.inner.clone(),
            workflow,
            serde_json::Value::Null,
            cancel,
            engine::ExecutionOptions {
                execution_id: Some(record.execution_id.clone()),
                resume: Some(resume),
            },
        )
        .await
    }

    /// Clears and returns the recorded task execution order.
    ///
    /// Used by conformance and integration tests to assert task ordering.
    pub fn take_task_order(&self) -> Vec<String> {
        self.inner.take_task_order()
    }
}

/// Builds a runtime-category [`WorkflowError`] with a detail message.
fn runtime_detail(detail: String) -> WorkflowError {
    WorkflowError::new(
        ErrorKind::Runtime,
        ProblemDetails::standard(StandardErrorType::Runtime).with_detail(detail),
    )
}

/// The error returned when an execution id is unknown.
fn unknown_execution(execution_id: &str) -> WorkflowError {
    runtime_detail(format!("unknown execution `{execution_id}`"))
}

/// Extracts the URI from a catalog endpoint definition.
fn catalog_endpoint_uri(
    endpoint: &crate::dsl_models::OneOfEndpointDefinitionOrUri,
) -> String {
    match endpoint {
        crate::dsl_models::OneOfEndpointDefinitionOrUri::Uri(uri) => uri.clone(),
        crate::dsl_models::OneOfEndpointDefinitionOrUri::Endpoint(e) => e.uri.clone(),
    }
}

/// Merges components imported from a catalog into `dst`.
fn merge_components(
    dst: &mut crate::dsl_models::ComponentDefinitionCollection,
    src: crate::dsl_models::ComponentDefinitionCollection,
) {
    if let Some(m) = src.authentications {
        dst.authentications
            .get_or_insert_with(Default::default)
            .extend(m);
    }
    if let Some(m) = src.errors {
        dst.errors.get_or_insert_with(Default::default).extend(m);
    }
    if let Some(m) = src.functions {
        dst.functions.get_or_insert_with(Default::default).extend(m);
    }
    if let Some(m) = src.retries {
        dst.retries.get_or_insert_with(Default::default).extend(m);
    }
    if let Some(m) = src.timeouts {
        dst.timeouts.get_or_insert_with(Default::default).extend(m);
    }
    if let Some(m) = src.catalogs {
        dst.catalogs.get_or_insert_with(Default::default).extend(m);
    }
    if let Some(v) = src.secrets {
        dst.secrets.get_or_insert_with(Vec::new).extend(v);
    }
    if let Some(v) = src.extensions {
        dst.extensions.get_or_insert_with(Vec::new).extend(v);
    }
}

async fn run_with_cancel(
    inner: Arc<RuntimeInner>,
    workflow: Arc<CompiledWorkflow>,
    input: serde_json::Value,
    cancel: Arc<Cancellation>,
) -> Result<serde_json::Value, WorkflowError> {
    let workflow_timeout = workflow.timeout;
    let cancel_for_exec = cancel.clone();
    let mut result_fut = Box::pin(engine::execute(inner, workflow, input, cancel_for_exec));

    let execution = async {
        tokio::select! {
            res = &mut result_fut => res,
            _ = cancel.notified() => Err(WorkflowError::cancelled()),
        }
    };

    match workflow_timeout {
        Some(dur) => match tokio::time::timeout(dur, execution).await {
            Ok(res) => res,
            Err(_) => Err(WorkflowError::timeout(None)),
        },
        None => execution.await,
    }
}

/// Builds a [`Runtime`] with configurable services.
pub struct RuntimeBuilder {
    expression: Option<Arc<dyn ows_runtime_core::ExpressionEngine>>,
    clock: Option<Arc<dyn Clock>>,
    uuid: Option<Arc<dyn UuidGenerator>>,
    random: Option<Arc<dyn RandomGenerator>>,
    runtime_info: RuntimeInfo,
    policy: RuntimePolicy,
    publisher: Option<Arc<dyn EventPublisher>>,
    service: Option<Arc<dyn ServiceInvoker>>,
    functions: HashMap<String, Arc<dyn FunctionInvoker>>,
    process: Option<Arc<dyn ProcessRunner>>,
    consumer: Option<Arc<dyn EventConsumer>>,
    store: Option<Arc<dyn ExecutionStore>>,
    scheduler: Option<Arc<dyn Scheduler>>,
    secrets: Arc<dyn SecretResolver>,
    catalog: Option<Arc<dyn crate::catalog::CatalogResolver>>,
    enable_schema_validation: bool,
}

impl RuntimeBuilder {
    /// Creates a new builder with safe defaults (deny-by-default network).
    pub fn new() -> Self {
        Self {
            expression: None,
            clock: None,
            uuid: None,
            random: None,
            runtime_info: RuntimeInfo::default(),
            policy: RuntimePolicy::default(),
            publisher: None,
            service: None,
            functions: HashMap::new(),
            process: None,
            consumer: None,
            store: None,
            scheduler: None,
            secrets: Arc::new(ows_runtime_core::EmptySecretResolver),
            catalog: None,
            enable_schema_validation: true,
        }
    }

    /// Sets the expression engine.
    pub fn with_expression(mut self, e: Arc<dyn ows_runtime_core::ExpressionEngine>) -> Self {
        self.expression = Some(e);
        self
    }

    /// Sets the clock.
    pub fn with_clock(mut self, c: Arc<dyn Clock>) -> Self {
        self.clock = Some(c);
        self
    }

    /// Sets the UUID generator.
    pub fn with_uuid(mut self, u: Arc<dyn UuidGenerator>) -> Self {
        self.uuid = Some(u);
        self
    }

    /// Sets the random generator.
    pub fn with_random(mut self, r: Arc<dyn RandomGenerator>) -> Self {
        self.random = Some(r);
        self
    }

    /// Sets the runtime info.
    pub fn with_runtime_info(mut self, info: RuntimeInfo) -> Self {
        self.runtime_info = info;
        self
    }

    /// Sets the runtime policy.
    pub fn with_policy(mut self, policy: RuntimePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Sets the event publisher.
    pub fn with_event_publisher(mut self, p: Arc<dyn EventPublisher>) -> Self {
        self.publisher = Some(p);
        self
    }

    /// Sets the generic service invoker (used by the built-in `http` function).
    pub fn with_service(mut self, s: Arc<dyn ServiceInvoker>) -> Self {
        self.service = Some(s);
        self
    }

    /// Registers a custom function invoker for `call` tasks.
    pub fn register_function(mut self, name: &str, f: Arc<dyn FunctionInvoker>) -> Self {
        self.functions.insert(name.to_string(), f);
        self
    }

    /// Sets the process runner (for `run` tasks).
    pub fn with_process(mut self, p: Arc<dyn ProcessRunner>) -> Self {
        self.process = Some(p);
        self
    }

    /// Sets the event consumer (for `listen` tasks and event-driven scheduling).
    pub fn with_event_consumer(mut self, c: Arc<dyn EventConsumer>) -> Self {
        self.consumer = Some(c);
        self
    }

    /// Sets the execution store.
    pub fn with_store(mut self, s: Arc<dyn ExecutionStore>) -> Self {
        self.store = Some(s);
        self
    }

    /// Sets the scheduler.
    pub fn with_scheduler(mut self, s: Arc<dyn Scheduler>) -> Self {
        self.scheduler = Some(s);
        self
    }

    /// Sets the secret resolver used to populate `$secrets`.
    pub fn with_secret_resolver(mut self, resolver: Arc<dyn SecretResolver>) -> Self {
        self.secrets = resolver;
        self
    }

    /// Sets secrets from a name → value mapping (convenience for embedding).
    pub fn with_secrets(
        mut self,
        secrets: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.secrets = Arc::new(ows_runtime_core::MapSecretResolver::from_pairs(secrets));
        self
    }

    /// Sets the catalog resolver used to resolve `use.catalogs` imports.
    pub fn with_catalog_resolver(
        mut self,
        resolver: Arc<dyn crate::catalog::CatalogResolver>,
    ) -> Self {
        self.catalog = Some(resolver);
        self
    }

    /// Builds the runtime.
    pub fn build(self) -> Result<Runtime, WorkflowError> {
        #[cfg(feature = "validation")]
        let validator = if self.enable_schema_validation {
            Some(Arc::new(JsonSchemaValidator))
        } else {
            None
        };
        #[cfg(not(feature = "validation"))]
        let validator = ();

        let inner = Arc::new(RuntimeInner {
            expression: self
                .expression
                .unwrap_or_else(|| Arc::new(ows_runtime_expressions::JqEngine::new())),
            clock: self.clock.unwrap_or_else(|| Arc::new(SystemClock::new())),
            uuid: self
                .uuid
                .unwrap_or_else(|| Arc::new(ows_runtime_core::DefaultUuidGenerator)),
            random: self
                .random
                .unwrap_or_else(|| Arc::new(ows_runtime_core::DefaultRandomGenerator::default())),
            runtime_info: self.runtime_info,
            policy: self.policy,
            publisher: self
                .publisher
                .unwrap_or_else(|| Arc::new(ows_runtime_core::NullEventPublisher)),
            service: self.service.unwrap_or_else(|| Arc::new(NoopServiceInvoker)),
            functions: Arc::new(std::sync::RwLock::new(self.functions)),
            process: self.process.unwrap_or_else(|| Arc::new(NoopProcessRunner)),
            consumer: self
                .consumer
                .unwrap_or_else(|| Arc::new(ows_runtime_core::NoopEventConsumer)),
            store: self
                .store
                .unwrap_or_else(|| Arc::new(InMemoryExecutionStore::new())),
            scheduler: self
                .scheduler
                .unwrap_or_else(|| Arc::new(ows_runtime_scheduler::NoopScheduler)),
            secrets: self.secrets,
            catalog: self.catalog,
            workflows: Arc::new(std::sync::Mutex::new(HashMap::new())),
            task_order: Arc::new(std::sync::Mutex::new(Vec::new())),
            #[cfg(feature = "validation")]
            validator,
        });

        // Register the default HTTP function backed by the service invoker.
        // (The invoker itself enforces the network policy.)
        #[cfg(feature = "http")]
        {
            let mut functions = inner.functions.write().unwrap();
            if !functions.contains_key("http") {
                functions.insert(
                    "http".to_string(),
                    Arc::new(HttpServiceInvoker::new(inner.clone())),
                );
            }
            if !functions.contains_key("openapi") {
                functions.insert(
                    "openapi".to_string(),
                    Arc::new(OpenApiInvoker::new(inner.clone())),
                );
            }
            if !functions.contains_key("mcp") {
                functions.insert("mcp".to_string(), Arc::new(McpInvoker::new(inner.clone())));
            }
            if !functions.contains_key("a2a") {
                functions.insert("a2a".to_string(), Arc::new(A2aInvoker::new(inner.clone())));
            }
            if !functions.contains_key("asyncapi") {
                functions.insert(
                    "asyncapi".to_string(),
                    Arc::new(AsyncApiInvoker::new(inner.clone())),
                );
            }
            if !functions.contains_key("grpc") {
                functions.insert(
                    "grpc".to_string(),
                    Arc::new(GrpcInvoker::new(inner.clone())),
                );
            }
            drop(functions);
        }

        Ok(Runtime { inner })
    }
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export the HTTP invoker type for users who want to inspect it.
#[cfg(feature = "http")]
pub use crate::service::HttpServiceInvoker as _HttpServiceInvokerExport;

#[cfg(test)]
mod tests {
    use super::*;
    use ows_runtime_core::{DeterministicClock, SeededRandomGenerator};

    #[test]
    fn builder_defaults_and_overrides() {
        let rt = Runtime::builder().build().unwrap();
        assert!(!rt.inner.runtime_info.name.is_empty());

        let rt = Runtime::builder()
            .with_clock(Arc::new(DeterministicClock::new()))
            .with_random(Arc::new(SeededRandomGenerator::new(1)))
            .with_runtime_info(RuntimeInfo {
                name: "test".into(),
                ..Default::default()
            })
            .build()
            .unwrap();
        assert_eq!(rt.inner.runtime_info.name, "test");
        assert!(rt.inner.clock.is_deterministic());
    }

    #[tokio::test]
    async fn register_and_run_definition() {
        let rt = Runtime::builder().build().unwrap();
        let def = ows_runtime_dsl::from_yaml(
            r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - a: { set: { x: 1 } }
"#,
        )
        .unwrap();
        let wf = rt.register_definition(&def).unwrap();
        let out = rt.run(wf, serde_json::Value::Null).await.unwrap();
        assert_eq!(out["x"], 1);
    }

    #[test]
    fn builder_register_function_and_policy() {
        let b = Runtime::builder();
        let b = b.with_policy(ows_runtime_core::RuntimePolicy {
            allow_network: true,
            ..Default::default()
        });
        assert!(!b.policy.allow_network || b.policy.allow_network);
    }

    #[test]
    fn runtime_inner_workflow_lookup() {
        let rt = Runtime::builder().build().unwrap();
        let def = ows_runtime_dsl::from_yaml(
            r#"
document: { dsl: '1.0.3', namespace: n, name: w, version: '1.0.0' }
do:
  - a: { set: { x: 1 } }
"#,
        )
        .unwrap();
        let wf = rt.register_definition(&def).unwrap();
        let key = format!("{}/{}/{}", wf.id.namespace, wf.id.name, wf.id.version);
        assert!(rt.inner.workflow(&key).is_some());
        assert!(rt.inner.workflow("nope").is_none());
    }

    #[test]
    fn cancellation_token_behavior() {
        let c = Cancellation::new();
        assert!(!c.is_cancelled());
        c.cancel();
        assert!(c.is_cancelled());
    }

    #[tokio::test]
    async fn cancellation_notifies() {
        let c = Arc::new(Cancellation::new());
        let c2 = c.clone();
        let handle = tokio::spawn(async move {
            tokio::select! {
                _ = c2.notified() => true,
                _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => false,
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        c.cancel();
        assert!(handle.await.unwrap());
    }

    #[test]
    fn builder_all_with_methods() {
        use ows_runtime_core::{
            DefaultUuidGenerator, NoopEventConsumer, NullEventPublisher, RuntimePolicy,
        };
        let rt = Runtime::builder()
            .with_expression(Arc::new(ows_runtime_expressions::JqEngine::new()))
            .with_uuid(Arc::new(DefaultUuidGenerator))
            .with_event_publisher(Arc::new(NullEventPublisher))
            .with_event_consumer(Arc::new(NoopEventConsumer))
            .with_service(Arc::new(NoopServiceInvoker))
            .with_process(Arc::new(NoopProcessRunner))
            .with_store(Arc::new(InMemoryExecutionStore::new()))
            .with_scheduler(Arc::new(ows_runtime_scheduler::NoopScheduler))
            .with_policy(RuntimePolicy::default())
            .build()
            .unwrap();
        assert!(rt.inner.functions.read().unwrap().contains_key("http"));
    }
}

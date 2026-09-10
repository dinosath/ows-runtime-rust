//! Secret resolution.
//!
//! Workflow definitions declare the secrets they consume via `use.secrets`.
//! The runtime resolves those names to values through a [`SecretResolver`] so
//! that secret material never has to appear in the workflow definition itself.
//! Only declared secret names are resolved and exposed as `$secrets` during
//! execution.

use std::collections::HashMap;

/// Resolves declared secret names to their values.
///
/// Implementations should be cheap and thread-safe. Returning `None` means the
/// secret is declared but unavailable; it is then exposed as `null`.
pub trait SecretResolver: Send + Sync {
    /// Resolves a secret by name.
    fn resolve(&self, name: &str) -> Option<String>;
}

/// A resolver that resolves nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptySecretResolver;

impl SecretResolver for EmptySecretResolver {
    fn resolve(&self, _name: &str) -> Option<String> {
        None
    }
}

/// A resolver backed by an in-memory name → value map. Useful for tests and for
/// embedding secret values directly.
#[derive(Debug, Clone, Default)]
pub struct MapSecretResolver {
    values: HashMap<String, String>,
}

impl MapSecretResolver {
    /// Creates a resolver from a name → value mapping.
    pub fn new(values: HashMap<String, String>) -> Self {
        Self { values }
    }

    /// Creates a resolver from an iterator of name/value pairs.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            values: pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
}

impl SecretResolver for MapSecretResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned()
    }
}

/// A resolver that reads secrets from environment variables.
///
/// A secret named `dbPassword` is read from `OWS_SECRET_DB_PASSWORD` (the name
/// is upper-cased and every non-alphanumeric character becomes `_`).
#[derive(Debug, Clone, Default)]
pub struct EnvSecretResolver {
    prefix: String,
}

impl EnvSecretResolver {
    /// Creates a resolver using the default `OWS_SECRET_` prefix.
    pub fn new() -> Self {
        Self {
            prefix: "OWS_SECRET_".to_string(),
        }
    }

    /// Creates a resolver using a custom environment variable prefix.
    pub fn with_prefix(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }

    /// Returns the environment variable name used for a secret.
    pub fn variable_for(&self, name: &str) -> String {
        let mut out = String::with_capacity(self.prefix.len() + name.len());
        out.push_str(&self.prefix);
        for c in name.chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_uppercase());
            } else {
                out.push('_');
            }
        }
        out
    }
}

impl SecretResolver for EnvSecretResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        std::env::var(self.variable_for(name)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_resolver_resolves_known_only() {
        let r = MapSecretResolver::from_pairs([("token", "s3cr3t")]);
        assert_eq!(r.resolve("token").as_deref(), Some("s3cr3t"));
        assert_eq!(r.resolve("missing"), None);
    }

    #[test]
    fn empty_resolver_resolves_nothing() {
        assert_eq!(EmptySecretResolver.resolve("x"), None);
    }

    #[test]
    fn env_resolver_builds_variable_names() {
        let r = EnvSecretResolver::new();
        assert_eq!(r.variable_for("dbPassword"), "OWS_SECRET_DBPASSWORD");
        assert_eq!(r.variable_for("db-password"), "OWS_SECRET_DB_PASSWORD");
        let r = EnvSecretResolver::with_prefix("X_");
        assert_eq!(r.variable_for("a.b"), "X_A_B");
    }

    #[test]
    fn env_resolver_reads_environment() {
        // Use a unique name so the test is independent of the ambient env.
        let name = "ows_test_env_secret";
        let var = EnvSecretResolver::new().variable_for(name);
        std::env::set_var(&var, "value-from-env");
        let r = EnvSecretResolver::new();
        assert_eq!(r.resolve(name).as_deref(), Some("value-from-env"));
        std::env::remove_var(&var);
    }
}

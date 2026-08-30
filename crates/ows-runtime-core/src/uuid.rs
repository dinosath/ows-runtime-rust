//! Injectable UUID and random number generation for deterministic execution.

use std::sync::Mutex;

use uuid::Uuid;

/// Generates UUIDs. Injected so that tests can produce deterministic identifiers.
pub trait UuidGenerator: Send + Sync {
    /// Generates a new v4 UUID.
    fn new_v4(&self) -> Uuid;
}

/// The default generator backed by `uuid::Uuid::new_v4`.
#[derive(Debug, Clone, Default)]
pub struct DefaultUuidGenerator;

impl UuidGenerator for DefaultUuidGenerator {
    fn new_v4(&self) -> Uuid {
        Uuid::new_v4()
    }
}

/// Generates random numbers. Injected so that jitter and other randomized
/// behavior can be made deterministic in tests.
pub trait RandomGenerator: Send + Sync {
    /// Returns a random `u64`.
    fn next_u64(&self) -> u64;

    /// Returns a random `f64` in the range `[0.0, 1.0)`.
    fn next_f64(&self) -> f64;
}

/// The default generator backed by the `rand` crate.
#[derive(Debug)]
pub struct DefaultRandomGenerator {
    rng: Mutex<rand::rngs::StdRng>,
}

impl Clone for DefaultRandomGenerator {
    fn clone(&self) -> Self {
        Self {
            rng: Mutex::new(self.rng.lock().unwrap().clone()),
        }
    }
}

impl Default for DefaultRandomGenerator {
    fn default() -> Self {
        use rand::SeedableRng;
        Self {
            rng: Mutex::new(rand::rngs::StdRng::from_entropy()),
        }
    }
}

impl RandomGenerator for DefaultRandomGenerator {
    fn next_u64(&self) -> u64 {
        use rand::Rng;
        self.rng.lock().unwrap().gen::<u64>()
    }

    fn next_f64(&self) -> f64 {
        use rand::Rng;
        self.rng.lock().unwrap().gen::<f64>()
    }
}

/// A deterministic random generator seeded with a fixed value.
#[derive(Debug)]
pub struct SeededRandomGenerator {
    rng: Mutex<rand::rngs::StdRng>,
}

impl Clone for SeededRandomGenerator {
    fn clone(&self) -> Self {
        Self {
            rng: Mutex::new(self.rng.lock().unwrap().clone()),
        }
    }
}

impl SeededRandomGenerator {
    /// Creates a new generator from the given seed.
    pub fn new(seed: u64) -> Self {
        use rand::SeedableRng;
        Self {
            rng: Mutex::new(rand::rngs::StdRng::seed_from_u64(seed)),
        }
    }
}

impl RandomGenerator for SeededRandomGenerator {
    fn next_u64(&self) -> u64 {
        use rand::Rng;
        self.rng.lock().unwrap().gen::<u64>()
    }

    fn next_f64(&self) -> f64 {
        use rand::Rng;
        self.rng.lock().unwrap().gen::<f64>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uuid_generator_produces_v4() {
        let g = DefaultUuidGenerator;
        let uuid = g.new_v4();
        assert_eq!(uuid.get_version_num(), 4);
    }

    #[test]
    fn seeded_random_generator_is_deterministic() {
        let a = SeededRandomGenerator::new(42);
        let b = SeededRandomGenerator::new(42);
        assert_eq!(a.next_u64(), b.next_u64());
    }
}

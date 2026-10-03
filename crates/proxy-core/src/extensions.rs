use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::{Arc, RwLock},
};

/// Exchange-local, type-indexed application state.
///
/// Values are reference counted so callers never hold a borrow across an
/// asynchronous hook boundary. An extensions store belongs to one exchange;
/// sharing state between exchanges is an explicit responsibility of the
/// interceptor factory.
#[derive(Clone, Default)]
pub struct ExchangeExtensions {
    values: Arc<RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>>,
}

impl std::fmt::Debug for ExchangeExtensions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExchangeExtensions")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl ExchangeExtensions {
    /// Creates an empty exchange-local store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or replaces one value by its concrete type.
    ///
    /// The previous value is returned when the same type was already present.
    pub fn insert<T>(&self, value: T) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.write()
            .insert(TypeId::of::<T>(), Arc::new(value))
            .and_then(|value| value.downcast::<T>().ok())
    }

    /// Returns a reference-counted value of type `T` when present.
    pub fn get<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.read()
            .get(&TypeId::of::<T>())
            .cloned()
            .and_then(|value| value.downcast::<T>().ok())
    }

    /// Removes and returns a value of type `T` when present.
    pub fn remove<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.write()
            .remove(&TypeId::of::<T>())
            .and_then(|value| value.downcast::<T>().ok())
    }

    /// Number of concrete value types stored for this exchange.
    pub fn len(&self) -> usize {
        self.read().len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<TypeId, Arc<dyn Any + Send + Sync>>> {
        self.values
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, HashMap<TypeId, Arc<dyn Any + Send + Sync>>> {
        self.values
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_values_by_type_without_cross_store_leakage() {
        let first = ExchangeExtensions::new();
        let second = ExchangeExtensions::new();
        assert!(first.insert(String::from("one")).is_none());
        assert_eq!(
            first.get::<String>().as_deref().map(String::as_str),
            Some("one")
        );
        assert!(second.get::<String>().is_none());
        assert_eq!(
            first
                .insert(String::from("two"))
                .as_deref()
                .map(String::as_str),
            Some("one")
        );
        assert_eq!(
            first.remove::<String>().as_deref().map(String::as_str),
            Some("two")
        );
        assert!(first.is_empty());
    }
}

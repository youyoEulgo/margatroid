use std::sync::Arc;
#[derive(Clone)]
pub struct UpdateProbe<T> {
    callback: Arc<dyn Fn(T) + Send + Sync>,
}
impl<T> std::fmt::Debug for UpdateProbe<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UpdateProbe")
    }
}
impl<T> UpdateProbe<T> {
    pub fn new(callback: impl Fn(T) + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }
    pub fn publish(&self, value: T) {
        (self.callback)(value)
    }
}

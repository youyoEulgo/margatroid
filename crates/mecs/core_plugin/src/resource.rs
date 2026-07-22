/// World 级单例。任何 'static + Send + Sync 类型自动实现。
pub trait Resource: 'static + Send + Sync {}
impl<T: 'static + Send + Sync> Resource for T {}

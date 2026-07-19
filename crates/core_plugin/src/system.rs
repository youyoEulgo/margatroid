use crate::world::World;

/// 对 World 执行操作的最小单元。
/// 任何 `FnMut(&mut World) + Send + 'static` 闭包自动实现此 trait。
pub trait System: Send + 'static {
    fn run(&mut self, world: &mut World);
    fn label(&self) -> Option<&'static str> {
        None
    }
    fn before(&self) -> &[&'static str] {
        &[]
    }
    fn after(&self) -> &[&'static str] {
        &[]
    }
}

// 让普通闭包也能直接当 System 用。
impl<F: FnMut(&mut World) + Send + 'static> System for F {
    fn run(&mut self, world: &mut World) {
        (self)(world)
    }
}

// 带标签和排序约束的 System 包装。
pub struct WrappedFn<F> {
    f: F,
    label: Option<&'static str>,
    before: Vec<&'static str>,
    after: Vec<&'static str>,
}

impl<F> WrappedFn<F> {
    pub fn before(mut self, label: &'static str) -> Self {
        self.before.push(label);
        self
    }

    pub fn after(mut self, label: &'static str) -> Self {
        self.after.push(label);
        self
    }
}

impl<F: FnMut(&mut World) + Send + 'static> System for WrappedFn<F> {
    fn run(&mut self, world: &mut World) {
        (self.f)(world)
    }
    fn label(&self) -> Option<&'static str> {
        self.label
    }
    fn before(&self) -> &[&'static str] {
        &self.before
    }
    fn after(&self) -> &[&'static str] {
        &self.after
    }
}

/// 创建一个命名 system，为后续 ordering 做准备。
pub fn named_system<F: FnMut(&mut World) + Send + 'static>(
    label: &'static str,
    f: F,
) -> WrappedFn<F> {
    WrappedFn {
        f,
        label: Some(label),
        before: Vec::new(),
        after: Vec::new(),
    }
}

/// 单个 System 执行失败后产生的框架事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemFailed {
    pub schedule: &'static str,
    pub system: Option<&'static str>,
    pub message: String,
}

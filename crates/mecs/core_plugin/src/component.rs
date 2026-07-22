use std::any::Any;

/// 实体上可挂载的数据。任何 'static + Send + Sync 类型自动实现。
pub trait Component: 'static + Send + Sync {}
impl<T: 'static + Send + Sync> Component for T {}

/// 一组 Component 可以一起 spawn。
pub trait Bundle {
    fn apply(self: Box<Self>, f: &mut dyn FnMut(Box<dyn Any + Send + Sync>));
}

// 空 Bundle
impl Bundle for () {
    fn apply(self: Box<Self>, _f: &mut dyn FnMut(Box<dyn Any + Send + Sync>)) {}
}

// 单个 Component
impl<A: Component> Bundle for (A,) {
    fn apply(self: Box<Self>, f: &mut dyn FnMut(Box<dyn Any + Send + Sync>)) {
        f(Box::new(self.0));
    }
}

// 二元组
impl<A: Component, B: Component> Bundle for (A, B) {
    fn apply(self: Box<Self>, f: &mut dyn FnMut(Box<dyn Any + Send + Sync>)) {
        let (a, b) = *self;
        f(Box::new(a));
        f(Box::new(b));
    }
}

// 三元组
impl<A: Component, B: Component, C: Component> Bundle for (A, B, C) {
    fn apply(self: Box<Self>, f: &mut dyn FnMut(Box<dyn Any + Send + Sync>)) {
        let (a, b, c) = *self;
        f(Box::new(a));
        f(Box::new(b));
        f(Box::new(c));
    }
}

// 四元组
impl<A: Component, B: Component, C: Component, D: Component> Bundle for (A, B, C, D) {
    fn apply(self: Box<Self>, f: &mut dyn FnMut(Box<dyn Any + Send + Sync>)) {
        let (a, b, c, d) = *self;
        f(Box::new(a));
        f(Box::new(b));
        f(Box::new(c));
        f(Box::new(d));
    }
}

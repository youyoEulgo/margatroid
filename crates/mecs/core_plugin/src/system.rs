use crate::World;

pub trait System: Send + 'static {
    fn run(&mut self, world: &mut World);
}

impl<F> System for F
where
    F: FnMut(&mut World) + Send + 'static,
{
    fn run(&mut self, world: &mut World) {
        self(world);
    }
}

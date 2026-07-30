use crate::App;

pub trait Plugin {
    fn build(self, app: &mut App);
}

impl<F> Plugin for F
where
    F: FnOnce(&mut App),
{
    fn build(self, app: &mut App) {
        self(app);
    }
}

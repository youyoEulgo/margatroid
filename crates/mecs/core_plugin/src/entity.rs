/// 带生成计数的实体 ID，防止复用问题。
/// 高 32 位：generation，低 32 位：index。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Entity(u64);

impl Entity {
    pub(crate) fn new(index: u32, generation: u32) -> Self {
        Entity(((generation as u64) << 32) | index as u64)
    }

    pub fn index(self) -> u32 {
        self.0 as u32
    }

    pub fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_index_and_generation() {
        let e = Entity::new(42, 3);
        assert_eq!(e.index(), 42);
        assert_eq!(e.generation(), 3);
    }
}

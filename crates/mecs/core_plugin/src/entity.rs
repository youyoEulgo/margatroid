use crate::CoreError;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Entity(u64);

impl Entity {
    pub(crate) fn new(index: u32, generation: u32) -> Self {
        Self(((generation as u64) << 32) | index as u64)
    }

    pub fn index(self) -> u32 {
        self.0 as u32
    }

    pub fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }
}

struct EntitySlot {
    generation: u32,
    alive: bool,
}

pub(crate) struct EntityAllocator {
    slots: Vec<EntitySlot>,
    free_indices: Vec<u32>,
    alive_count: usize,
}

impl EntityAllocator {
    pub(crate) fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_indices: Vec::new(),
            alive_count: 0,
        }
    }

    pub(crate) fn allocate(&mut self) -> Entity {
        let index = if let Some(index) = self.free_indices.pop() {
            self.slots[index as usize].alive = true;
            index
        } else {
            let index = u32::try_from(self.slots.len())
                .unwrap_or_else(|_| CoreError::EntityCapacityExhausted.panic());
            self.slots.push(EntitySlot {
                generation: 0,
                alive: true,
            });
            index
        };

        self.alive_count += 1;
        Entity::new(index, self.slots[index as usize].generation)
    }

    pub(crate) fn release(&mut self, entity: Entity) -> bool {
        if !self.is_alive(entity) {
            return false;
        }

        let slot = &mut self.slots[entity.index() as usize];
        slot.alive = false;
        self.alive_count -= 1;
        if let Some(generation) = slot.generation.checked_add(1) {
            slot.generation = generation;
            self.free_indices.push(entity.index());
        }
        true
    }

    pub(crate) fn is_alive(&self, entity: Entity) -> bool {
        self.slots
            .get(entity.index() as usize)
            .is_some_and(|slot| slot.alive && slot.generation == entity.generation())
    }

    pub(crate) fn len(&self) -> usize {
        self.alive_count
    }

    pub(crate) fn iter_alive(&self) -> impl Iterator<Item = Entity> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.alive)
            .map(|(index, slot)| Entity::new(index as u32, slot.generation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_indices_are_reused_with_a_new_generation() {
        let mut allocator = EntityAllocator::new();
        let old = allocator.allocate();

        assert!(allocator.release(old));
        let new = allocator.allocate();

        assert_eq!(old.index(), new.index());
        assert_ne!(old.generation(), new.generation());
        assert!(!allocator.is_alive(old));
        assert!(allocator.is_alive(new));
    }

    #[test]
    fn alive_entities_are_iterated() {
        let mut allocator = EntityAllocator::new();
        let first = allocator.allocate();
        let removed = allocator.allocate();
        let last = allocator.allocate();
        allocator.release(removed);

        assert_eq!(allocator.iter_alive().collect::<Vec<_>>(), [first, last]);
        assert_eq!(allocator.len(), 2);
    }

    #[test]
    fn generation_overflow_permanently_retires_the_index() {
        let mut allocator = EntityAllocator::new();
        allocator.allocate();
        allocator.slots[0].generation = u32::MAX;
        let retired = Entity::new(0, u32::MAX);

        assert!(allocator.release(retired));
        let next = allocator.allocate();

        assert_eq!(next.index(), 1);
        assert!(!allocator.is_alive(retired));
    }
}

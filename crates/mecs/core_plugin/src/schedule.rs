use crate::{System, World};

pub struct Schedule {
    systems: Vec<Box<dyn System>>,
}

impl Schedule {
    pub fn new() -> Self {
        Self {
            systems: Vec::new(),
        }
    }

    pub fn add_system<S: System>(&mut self, system: S) -> &mut Self {
        self.systems.push(Box::new(system));
        self
    }

    pub fn run(&mut self, world: &mut World) {
        for system in &mut self.systems {
            system.run(world);
        }
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

type RunPlan = fn(&mut SchedulePlan, &mut World);

pub(crate) struct SchedulePlan {
    first_plan: Vec<(bool, usize)>,
    once: Vec<(String, Schedule)>,
    recurring: Vec<(String, Schedule)>,
    started: bool,
    run_plan: RunPlan,
}

impl SchedulePlan {
    pub(crate) fn new() -> Self {
        Self {
            first_plan: Vec::new(),
            once: Vec::new(),
            recurring: Vec::new(),
            started: false,
            run_plan: Self::first_run,
        }
    }

    pub(crate) fn add_schedule(&mut self, name: String) -> bool {
        if self.started || self.contains(&name) {
            return false;
        }
        self.first_plan.push((false, self.recurring.len()));
        self.recurring.push((name, Schedule::new()));
        true
    }

    pub(crate) fn add_once_schedule(&mut self, name: String) -> bool {
        if self.started || self.contains(&name) {
            return false;
        }
        self.first_plan.push((true, self.once.len()));
        self.once.push((name, Schedule::new()));
        true
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.once.iter().any(|(current, _)| current == name)
            || self.recurring.iter().any(|(current, _)| current == name)
    }

    pub(crate) fn is_started(&self) -> bool {
        self.started
    }

    pub(crate) fn schedule_mut(&mut self, name: &str) -> Option<&mut Schedule> {
        if self.started {
            return None;
        }
        self.once
            .iter_mut()
            .chain(&mut self.recurring)
            .find(|(current, _)| current == name)
            .map(|(_, schedule)| schedule)
    }

    fn first_run(&mut self, world: &mut World) {
        for &(is_once, index) in &self.first_plan {
            if is_once {
                self.once[index].1.run(world);
            } else {
                self.recurring[index].1.run(world);
            }
        }
        self.first_plan.clear();
        self.once.clear();
        self.started = true;
        self.run_plan = Self::continued_run;
    }

    fn continued_run(&mut self, world: &mut World) {
        for (_, schedule) in &mut self.recurring {
            schedule.run(world);
        }
    }

    pub(crate) fn run(&mut self, world: &mut World) {
        (self.run_plan)(self, world);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn first_run_preserves_mixed_registration_order() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut plan = SchedulePlan::new();
        plan.add_schedule("first".into());
        plan.add_once_schedule("startup".into());
        plan.add_schedule("last".into());

        for (name, value) in [("first", 1), ("startup", 2), ("last", 3)] {
            let calls = Arc::clone(&calls);
            plan.schedule_mut(name)
                .unwrap()
                .add_system(move |_world: &mut World| calls.lock().unwrap().push(value));
        }

        let mut world = World::new();
        plan.run(&mut world);
        plan.run(&mut world);

        assert_eq!(*calls.lock().unwrap(), [1, 2, 3, 1, 3]);
    }
}

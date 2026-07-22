use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::system::System;
use crate::world::World;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemRunFailure {
    pub system: Option<&'static str>,
    pub message: String,
}

#[derive(Default)]
pub struct ScheduleReport {
    pub failures: Vec<SystemRunFailure>,
    pub ordering_error: Option<String>,
}

impl ScheduleReport {
    pub fn is_ok(&self) -> bool {
        self.failures.is_empty() && self.ordering_error.is_none()
    }
}

/// 有序的 system 集合。无约束的 system 保持注册顺序。
pub struct Schedule {
    systems: Vec<Box<dyn System>>,
    ordered: bool,
}

impl Schedule {
    pub fn new() -> Self {
        Self {
            systems: Vec::new(),
            ordered: true,
        }
    }

    pub fn add_system(&mut self, system: impl System) -> &mut Self {
        self.systems.push(Box::new(system));
        self.ordered = false;
        self
    }

    /// 按依赖顺序执行，并隔离单个 system 的 panic。
    pub fn run(&mut self, world: &mut World) -> ScheduleReport {
        if let Err(error) = self.ensure_ordered() {
            tracing::error!(error = %error, "system ordering failed");
            return ScheduleReport {
                failures: Vec::new(),
                ordering_error: Some(error),
            };
        }

        let mut failures = Vec::new();
        for system in &mut self.systems {
            let label = system.label();
            tracing::debug!(system = label.unwrap_or("<anonymous>"), "system started");
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| system.run(world))) {
                let message = panic_message(payload);
                tracing::error!(system = label.unwrap_or("<anonymous>"), %message, "system panicked");
                failures.push(SystemRunFailure {
                    system: label,
                    message,
                });
            }
        }
        ScheduleReport {
            failures,
            ordering_error: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    fn ensure_ordered(&mut self) -> Result<(), String> {
        if self.ordered {
            return Ok(());
        }

        let count = self.systems.len();
        let mut labels = HashMap::new();
        for (index, system) in self.systems.iter().enumerate() {
            if let Some(label) = system.label() {
                if labels.insert(label, index).is_some() {
                    return Err(format!("duplicate system label `{label}`"));
                }
            }
        }

        let mut edges = vec![HashSet::new(); count];
        let mut indegree = vec![0_usize; count];
        for (index, system) in self.systems.iter().enumerate() {
            for target in system.before() {
                let target_index = *labels
                    .get(target)
                    .ok_or_else(|| format!("unknown system label `{target}`"))?;
                add_edge(index, target_index, &mut edges, &mut indegree);
            }
            for target in system.after() {
                let target_index = *labels
                    .get(target)
                    .ok_or_else(|| format!("unknown system label `{target}`"))?;
                add_edge(target_index, index, &mut edges, &mut indegree);
            }
        }

        let mut order = Vec::with_capacity(count);
        let mut emitted = vec![false; count];
        while order.len() < count {
            let Some(next) = (0..count).find(|&index| !emitted[index] && indegree[index] == 0)
            else {
                return Err("system ordering contains a cycle".into());
            };
            emitted[next] = true;
            order.push(next);
            for &dependent in &edges[next] {
                indegree[dependent] -= 1;
            }
        }

        let mut systems: Vec<_> = std::mem::take(&mut self.systems)
            .into_iter()
            .map(Some)
            .collect();
        self.systems = order
            .into_iter()
            .map(|index| systems[index].take().expect("system already reordered"))
            .collect();
        self.ordered = true;
        Ok(())
    }
}

fn add_edge(from: usize, to: usize, edges: &mut [HashSet<usize>], indegree: &mut [usize]) {
    if edges[from].insert(to) {
        indegree[to] += 1;
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "system panicked with a non-string payload".into()
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::named_system;

    #[test]
    fn ordering_constraints_override_registration_order() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let first_calls = calls.clone();
        let second_calls = calls.clone();
        let mut schedule = Schedule::new();
        schedule.add_system(
            named_system("second", move |_world| second_calls.lock().unwrap().push(2))
                .after("first"),
        );
        schedule.add_system(named_system("first", move |_world| {
            first_calls.lock().unwrap().push(1)
        }));

        let report = schedule.run(&mut World::new());

        assert!(report.is_ok());
        assert_eq!(*calls.lock().unwrap(), [1, 2]);
    }

    #[test]
    fn panic_does_not_prevent_later_systems() {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let later_ran = ran.clone();
        let mut schedule = Schedule::new();
        schedule.add_system(named_system("panic", |_world| panic!("boom")));
        schedule.add_system(move |_world: &mut World| {
            later_ran.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let report = schedule.run(&mut World::new());

        assert_eq!(report.failures[0].message, "boom");
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn ordering_cycle_is_reported_without_running_systems() {
        let mut schedule = Schedule::new();
        schedule.add_system(named_system("first", |_world| {}).after("second"));
        schedule.add_system(named_system("second", |_world| {}).after("first"));

        let report = schedule.run(&mut World::new());

        assert_eq!(
            report.ordering_error.as_deref(),
            Some("system ordering contains a cycle")
        );
    }
}

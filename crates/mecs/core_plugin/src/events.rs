use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::Mutex;

/// 可由 App 分发的事件。
pub trait Event: Clone + Send + Sync + 'static {}
impl<T: Clone + Send + Sync + 'static> Event for T {}

struct StoredEvent<E> {
    id: u64,
    frame: u64,
    value: E,
}

struct EventsInner<E> {
    events: VecDeque<StoredEvent<E>>,
    next_id: u64,
    frame: u64,
}

/// 事件读取器。内部游标对调用方透明，可以作为 system 状态跨帧保存。
pub struct EventReader<E: Event> {
    next_id: u64,
    missed: u64,
    _marker: PhantomData<E>,
}

/// 每种事件类型在 World 中对应一个内部队列。
///
/// 该类型不对 crate 外公开，调用方只通过 App 和 World 的事件 API 使用它。
pub(crate) struct Events<E: Event> {
    inner: Mutex<EventsInner<E>>,
    retention_frames: u64,
}

impl<E: Event> Events<E> {
    pub(crate) fn new(retention_frames: u64) -> Self {
        assert!(retention_frames > 0, "event retention must be positive");
        Self {
            inner: Mutex::new(EventsInner {
                events: VecDeque::new(),
                next_id: 0,
                frame: 0,
            }),
            retention_frames,
        }
    }

    pub(crate) fn reader(&self) -> EventReader<E> {
        let inner = self.lock();
        EventReader {
            next_id: inner.next_id,
            missed: 0,
            _marker: PhantomData,
        }
    }

    pub(crate) fn send(&self, event: E) {
        let mut inner = self.lock();
        let stored = StoredEvent {
            id: inner.next_id,
            frame: inner.frame,
            value: event,
        };
        inner.next_id = inner.next_id.wrapping_add(1);
        inner.events.push_back(stored);
    }

    pub(crate) fn read(&self, reader: &mut EventReader<E>) -> Vec<E> {
        let inner = self.lock();
        if let Some(oldest) = inner.events.front() {
            if reader.next_id < oldest.id {
                reader.missed = reader
                    .missed
                    .wrapping_add(oldest.id.wrapping_sub(reader.next_id));
                reader.next_id = oldest.id;
            }
        }
        let events = inner
            .events
            .iter()
            .filter(|event| event.id >= reader.next_id)
            .map(|event| event.value.clone())
            .collect();
        reader.next_id = inner.next_id;
        events
    }

    /// 推进帧号并清理超过保留期限的事件。
    pub(crate) fn finish_frame(&self) {
        let mut inner = self.lock();
        inner.frame = inner.frame.wrapping_add(1);
        let frame = inner.frame;
        while inner
            .events
            .front()
            .is_some_and(|event| event.frame.wrapping_add(self.retention_frames) <= frame)
        {
            inner.events.pop_front();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, EventsInner<E>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<E: Event> EventReader<E> {
    /// 返回该 reader 因事件过期而累计漏掉的事件数。
    pub fn missed_events(&self) -> u64 {
        self.missed
    }

    /// 取出并清零累计漏读数。
    pub fn take_missed_events(&mut self) -> u64 {
        std::mem::take(&mut self.missed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_skips_events_that_existed_when_it_was_created() {
        let events = Events::<i32>::new(2);
        events.send(1);

        let mut reader = events.reader();

        assert!(events.read(&mut reader).is_empty());
    }

    #[test]
    fn readers_consume_events_independently() {
        let events = Events::<String>::new(2);
        let mut first = events.reader();
        let mut second = events.reader();
        events.send("a".into());
        events.send("b".into());

        assert_eq!(events.read(&mut first), ["a", "b"]);
        assert_eq!(events.read(&mut second), ["a", "b"]);
        assert!(events.read(&mut first).is_empty());
    }

    #[test]
    fn reader_can_read_an_event_during_the_next_frame() {
        let events = Events::<i32>::new(2);
        let mut reader = events.reader();
        events.send(1);
        events.finish_frame();

        assert_eq!(events.read(&mut reader), [1]);
        assert_eq!(reader.missed_events(), 0);
    }

    #[test]
    fn reader_reports_events_that_expired() {
        let events = Events::<i32>::new(2);
        let mut reader = events.reader();
        events.send(1);
        events.finish_frame();
        events.finish_frame();
        events.send(2);

        assert_eq!(events.read(&mut reader), [2]);
        assert_eq!(reader.take_missed_events(), 1);
        assert_eq!(reader.missed_events(), 0);
    }

    #[test]
    fn expired_events_are_pruned_during_long_runs() {
        let events = Events::<u64>::new(2);
        let mut reader = events.reader();

        for value in 0..10_000 {
            events.send(value);
            assert_eq!(events.read(&mut reader), [value]);
            events.finish_frame();
        }

        assert!(events.lock().events.len() <= 2);
        assert_eq!(reader.missed_events(), 0);
    }
}

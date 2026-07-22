//! 消息刷新门控
/// 控制消息写入的门控状态机
///
/// 在 bridge session 启动时，历史消息通过 HTTP POST 批量发送。
/// 发送期间新到达的消息必须排队，防止与历史消息交错。
pub struct FlushGate<T> {
    active: bool,
    pending: Vec<T>,
}

impl<T> FlushGate<T> {
    pub fn new() -> Self {
        Self {
            active: false,
            pending: Vec::new(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// 标记 flush 开始，后续 enqueue() 会排队
    pub fn start(&mut self) {
        self.active = true;
    }

    /// 结束 flush，返回所有排队项（调用方负责发送）
    pub fn end(&mut self) -> Vec<T> {
        self.active = false;
        std::mem::take(&mut self.pending)
    }

    /// 如果 flush 激活则排队并返回 true，否则返回 false
    pub fn enqueue(&mut self, items: impl IntoIterator<Item = T>) -> bool {
        if !self.active {
            return false;
        }
        self.pending.extend(items);
        true
    }

    /// 丢弃所有排队项（永久关闭 transport 时使用），返回丢弃数量
    pub fn drop_pending(&mut self) -> usize {
        self.active = false;
        let count = self.pending.len();
        self.pending.clear();
        count
    }

    /// 仅清除 active 标志，不丢弃排队项
    ///
    /// transport 被替换时使用——新 transport 的 flush 会排空这些项
    pub fn deactivate(&mut self) {
        self.active = false;
    }
}

impl<T> Default for FlushGate<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flush_gate_basic() {
        let mut gate: FlushGate<i32> = FlushGate::new();
        assert!(!gate.is_active());

        gate.start();
        assert!(gate.is_active());

        assert!(gate.enqueue([1, 2, 3]));
        assert_eq!(gate.pending_count(), 3);

        let items = gate.end();
        assert_eq!(items, vec![1, 2, 3]);
        assert!(!gate.is_active());
        assert_eq!(gate.pending_count(), 0);
    }

    #[test]
    fn test_flush_gate_drop() {
        let mut gate: FlushGate<i32> = FlushGate::new();
        gate.start();
        gate.enqueue([1, 2]);
        let dropped = gate.drop_pending();
        assert_eq!(dropped, 2);
        assert!(!gate.is_active());
        assert_eq!(gate.pending_count(), 0);
    }

    #[test]
    fn test_flush_gate_not_active() {
        let mut gate: FlushGate<i32> = FlushGate::new();
        // 未激活时 enqueue 返回 false
        assert!(!gate.enqueue([1]));
        assert_eq!(gate.pending_count(), 0);
    }
}

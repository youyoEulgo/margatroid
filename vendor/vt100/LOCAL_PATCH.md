# Local vt100 patch

Upstream: vt100 0.16.2, MIT, copied from the published crate. Runtime sources and license are preserved. The manifest retains runtime dependencies; upstream development dependencies and unpublished test fixtures are omitted.

Local change: `Screen::set_scrollback_len` adjusts the existing normal-screen grid capacity, evicts excess oldest rows, shrinks the deque and clamps the view offset. It preserves the parser, both screen buffers, cursor, modes and incomplete escape/UTF-8 state. The alternate grid retains its upstream zero-scrollback behavior.

The shell client sets this limit to twice the actual terminal height on creation and resize. Regression coverage is in the shell-client output tests.

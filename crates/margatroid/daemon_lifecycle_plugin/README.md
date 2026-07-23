# DaemonLifecyclePlugin

Margatroid-specific daemon lifecycle and readiness policy.

The plugin exposes `Starting`, `Ready`, `Draining`, and `Stopped`, and registers `/ready` on the
shared HTTP server. Signal handling and infrastructure ownership remain in their respective mecs
plugins.

use crate::app::App;

/// 功能组合单元。打包一组 System + Component 注册逻辑。
pub trait Plugin {
    fn build(&self, app: &mut App);
}

/// 一组 Plugin，可一次性插入 App。
pub struct PluginGroup {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginGroup {
    pub fn new() -> Self {
        PluginGroup {
            plugins: Vec::new(),
        }
    }

    pub fn add_plugin<P: Plugin + 'static>(mut self, plugin: P) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    pub fn build_all(self, app: &mut App) {
        for plugin in self.plugins {
            plugin.build(app);
        }
    }
}

impl Default for PluginGroup {
    fn default() -> Self {
        PluginGroup::new()
    }
}

// 允许单个 Plugin 直接传入 add_plugins
impl<P: Plugin + 'static> From<P> for PluginGroup {
    fn from(plugin: P) -> Self {
        PluginGroup::new().add_plugin(plugin)
    }
}

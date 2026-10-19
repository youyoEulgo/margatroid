use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use margatroid_types::discovery::{
    ProjectDirectoryBrowserHandle, QueryProjectDirectories, QueryProjectDirectoriesResult,
};

pub(crate) fn system(world: &mut World) {
    let requests = world
        .event_reader::<QueryProjectDirectories>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        let Some(browser) = world
            .get_resource::<ProjectDirectoryBrowserHandle>()
            .cloned()
        else {
            world.send_event(QueryProjectDirectoriesResult {
                id: request.id,
                result: Err("Project directory browsing is not installed".into()),
            });
            continue;
        };
        let sender = world.event_sender();
        world.spawn_async_service(async move {
            let result =
                tokio::task::spawn_blocking(move || browser.0.query(request.path, request.cursor))
                    .await
                    .unwrap_or_else(|_| Err("Project directory browser worker failed".into()));
            sender.send_event(QueryProjectDirectoriesResult {
                id: request.id,
                result,
            });
        });
    }
}

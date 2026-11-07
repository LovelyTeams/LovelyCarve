//! Backend events on the UI thread. The hub entity re-emits every event it
//! receives from the backend channel; models subscribe to it and pick the
//! events they own by name.
use futures::{channel::mpsc::UnboundedReceiver, StreamExt as _};
use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global};

use crate::backend::BackendEvent;

/// Re-emits backend events on the UI thread.
pub struct EventHub;

impl EventEmitter<BackendEvent> for EventHub {}

struct GlobalEventHub(Entity<EventHub>);

impl Global for GlobalEventHub {}

impl EventHub {
    /// Create the hub and forward `receiver` to it for the application's
    /// lifetime.
    pub fn install(mut receiver: UnboundedReceiver<BackendEvent>, cx: &mut App) {
        let hub = cx.new(|_| EventHub);
        cx.set_global(GlobalEventHub(hub.clone()));
        let hub = hub.downgrade();
        cx.spawn(async move |cx| {
            while let Some(event) = receiver.next().await {
                if hub.update(cx, |_, cx| cx.emit(event)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalEventHub>().0.clone()
    }
}

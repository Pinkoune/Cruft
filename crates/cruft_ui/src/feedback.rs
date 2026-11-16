use bevy::prelude::*;
use cruft_core::EditorEvent;
use std::collections::VecDeque;

/// Keeps immediate core events until the UI's next render pass.
/// The layout consumes this queue to display animated toasts.
#[derive(Resource, Default)]
pub struct ToastQueue(VecDeque<String>);

impl ToastQueue {
    pub const LIMIT: usize = 32;

    pub fn pending(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    pub fn drain(&mut self) -> impl Iterator<Item = String> + '_ {
        self.0.drain(..)
    }
}

pub(crate) fn collect_toasts(event: On<EditorEvent>, mut queue: ResMut<ToastQueue>) {
    let EditorEvent::Toast(message) = event.event();
    if queue.0.len() == ToastQueue::LIMIT {
        queue.0.pop_front();
    }
    queue.0.push_back(message.clone());
}

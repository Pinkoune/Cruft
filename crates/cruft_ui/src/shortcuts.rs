//! Preserve modifier ordering when a shortcut is pressed and released in one
//! frame. bevy_egui 0.43 annotates all key events with the final modifier state.
use bevy::{
    input::keyboard::{Key, KeyboardFocusLost, KeyboardInput},
    prelude::*,
};

#[derive(Resource, Default)]
pub(crate) struct Shortcuts {
    pub undo: bool,
    pub redo: bool,
    pub play: bool,
    pub palette: bool,
    pub save: bool,
}

#[derive(Default)]
pub(crate) struct HeldModifiers {
    control: bool,
    shift: bool,
}

pub(crate) fn collect(
    mut events: MessageReader<KeyboardInput>,
    mut lost: MessageReader<KeyboardFocusLost>,
    mut held: Local<HeldModifiers>,
    mut shortcuts: ResMut<Shortcuts>,
) {
    if !lost.is_empty() {
        lost.clear();
        *held = HeldModifiers::default();
    }
    for event in events.read() {
        match &event.logical_key {
            Key::Control => held.control = event.state.is_pressed(),
            Key::Shift => held.shift = event.state.is_pressed(),
            Key::Character(key) if held.control && event.state.is_pressed() && !event.repeat => {
                match key.to_ascii_lowercase().as_str() {
                    "z" if held.shift => shortcuts.redo = true,
                    "z" => shortcuts.undo = true,
                    "y" => shortcuts.redo = true,
                    "p" => shortcuts.play = true,
                    "k" => shortcuts.palette = true,
                    "s" => shortcuts.save = true,
                    _ => (),
                }
            }
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;

    #[test]
    fn rapid_control_shortcut_retains_modifiers_and_does_not_stick() {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<KeyboardFocusLost>()
            .init_resource::<Shortcuts>()
            .add_systems(Update, collect);
        for (key, code, state) in [
            (Key::Control, KeyCode::ControlLeft, ButtonState::Pressed),
            (
                Key::Character("z".into()),
                KeyCode::KeyZ,
                ButtonState::Pressed,
            ),
            (
                Key::Character("z".into()),
                KeyCode::KeyZ,
                ButtonState::Released,
            ),
            (Key::Control, KeyCode::ControlLeft, ButtonState::Released),
        ] {
            app.world_mut().write_message(KeyboardInput {
                key_code: code,
                logical_key: key,
                state,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
        }
        app.update();
        assert!(app.world().resource::<Shortcuts>().undo);
        *app.world_mut().resource_mut::<Shortcuts>() = Shortcuts::default();
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::KeyP,
            logical_key: Key::Character("p".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert!(!app.world().resource::<Shortcuts>().play);
        assert!(!app.world().resource::<Shortcuts>().undo);
    }
}

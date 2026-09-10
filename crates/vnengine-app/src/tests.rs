#[cfg(test)]
mod tests {
    use crate::input::{key_to_action, Action, Input};
    use winit::keyboard::KeyCode;

    #[test]
    fn input_advance_keys() {
        assert_eq!(key_to_action(KeyCode::Space), Some(Action::Advance));
        assert_eq!(key_to_action(KeyCode::Enter), Some(Action::Advance));
        assert_eq!(key_to_action(KeyCode::NumpadEnter), Some(Action::Advance));
    }

    #[test]
    fn input_cancel_keys() {
        assert_eq!(key_to_action(KeyCode::Escape), Some(Action::Cancel));
        assert_eq!(key_to_action(KeyCode::Backspace), Some(Action::Cancel));
    }

    #[test]
    fn input_function_keys() {
        assert_eq!(key_to_action(KeyCode::F5), Some(Action::Save));
        assert_eq!(key_to_action(KeyCode::F9), Some(Action::Load));
        assert_eq!(key_to_action(KeyCode::F11), Some(Action::Fullscreen));
        assert_eq!(key_to_action(KeyCode::F1), Some(Action::Settings));
    }

    #[test]
    fn input_toggle_keys() {
        assert_eq!(key_to_action(KeyCode::KeyA), Some(Action::AutoAdvance));
        assert_eq!(key_to_action(KeyCode::KeyS), Some(Action::Skip));
    }

    #[test]
    fn input_quit_keys() {
        assert_eq!(key_to_action(KeyCode::F4), Some(Action::Quit));
        assert_eq!(key_to_action(KeyCode::KeyQ), Some(Action::Quit));
    }

    #[test]
    fn input_unmapped_key() {
        assert_eq!(key_to_action(KeyCode::KeyZ), None);
        assert_eq!(key_to_action(KeyCode::Tab), None);
    }

    #[test]
    fn input_press_release_cycle() {
        let mut input = Input::new();
        input.begin_frame();
        assert!(!input.pressed(Action::Advance));
        assert!(!input.held(Action::Advance));

        input.key_press(KeyCode::Space);
        assert!(input.pressed(Action::Advance));
        assert!(input.held(Action::Advance));

        // Next frame: pressed should clear, held should persist.
        input.begin_frame();
        assert!(!input.pressed(Action::Advance));
        assert!(input.held(Action::Advance));

        input.key_release(KeyCode::Space);
        assert!(!input.pressed(Action::Advance));
        assert!(!input.held(Action::Advance));
    }

    #[test]
    fn input_advance_requested() {
        let mut input = Input::new();
        input.begin_frame();
        assert!(!input.advance_requested());

        input.key_press(KeyCode::Enter);
        assert!(input.advance_requested());

        input.begin_frame();
        assert!(!input.advance_requested());

        // Mouse click also triggers advance.
        input.mouse_click(100.0, 200.0);
        assert!(input.advance_requested());
    }

    #[test]
    fn input_mouse_pos() {
        let mut input = Input::new();
        input.mouse_move(50.0, 75.0);
        assert_eq!(input.mouse_pos(), (50.0, 75.0));
    }

    #[test]
    fn input_multiple_clicks() {
        let mut input = Input::new();
        input.begin_frame();
        input.mouse_click(10.0, 20.0);
        input.mouse_click(30.0, 40.0);
        let clicks = input.clicks();
        assert_eq!(clicks.len(), 2);
        assert_eq!(clicks[0].x, 10.0);
        assert_eq!(clicks[1].x, 30.0);
    }

    #[test]
    fn input_held_across_frames() {
        let mut input = Input::new();
        input.begin_frame();
        input.key_press(KeyCode::KeyA);
        assert!(input.held(Action::AutoAdvance));

        input.begin_frame();
        // Not pressed again this frame, but still held.
        assert!(!input.pressed(Action::AutoAdvance));
        assert!(input.held(Action::AutoAdvance));

        input.begin_frame();
        assert!(input.held(Action::AutoAdvance));

        input.key_release(KeyCode::KeyA);
        assert!(!input.held(Action::AutoAdvance));
    }
}

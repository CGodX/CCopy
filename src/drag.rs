use mouse_position::mouse_position::Mouse;

#[derive(Clone, Copy)]
pub struct DragState {
    pub window_x: i32,
    pub window_y: i32,
    pub cursor_x: i32,
    pub cursor_y: i32,
}

pub fn cursor_position() -> Option<(i32, i32)> {
    match Mouse::get_mouse_position() {
        Mouse::Position { x, y } => Some((x, y)),
        Mouse::Error => None,
    }
}
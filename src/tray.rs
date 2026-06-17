use std::error::Error;

use tray_icon::Icon;

pub fn create_tray_icon() -> Result<Icon, Box<dyn Error>> {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);

    for y in 0..size {
        for x in 0..size {
            let in_mark = (8..=23).contains(&x) && (8..=11).contains(&y)
                || (8..=11).contains(&x) && (8..=23).contains(&y)
                || (8..=23).contains(&x) && (20..=23).contains(&y);

            if in_mark {
                rgba.extend_from_slice(&[255, 255, 255, 255]);
            } else {
                rgba.extend_from_slice(&[83, 112, 255, 255]);
            }
        }
    }

    Ok(Icon::from_rgba(rgba, size, size)?)
}

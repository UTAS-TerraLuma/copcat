//! Names and colours for the LAS point classes (ASPRS LAS 1.4, table 17).

/// Classes the LAS spec names. 19..=63 are reserved, 64..=255 user defined.
const NAMED: [(&str, [f32; 3]); 19] = [
    ("Never classified", [0.45, 0.45, 0.45]),
    ("Unclassified", [0.7, 0.7, 0.7]),
    ("Ground", [0.64, 0.47, 0.3]),
    ("Low vegetation", [0.65, 0.88, 0.4]),
    ("Medium vegetation", [0.3, 0.72, 0.25]),
    ("High vegetation", [0.1, 0.45, 0.15]),
    ("Building", [0.9, 0.33, 0.25]),
    ("Low noise", [1.0, 0.0, 1.0]),
    ("Model key-point", [1.0, 0.95, 0.4]),
    ("Water", [0.2, 0.45, 0.95]),
    ("Rail", [0.55, 0.3, 0.6]),
    ("Road surface", [0.32, 0.32, 0.36]),
    ("Overlap", [0.85, 0.85, 0.55]),
    ("Wire guard", [1.0, 0.85, 0.0]),
    ("Wire conductor", [1.0, 0.55, 0.0]),
    ("Transmission tower", [0.8, 0.2, 0.55]),
    ("Wire connector", [0.0, 0.85, 0.85]),
    ("Bridge deck", [0.7, 0.6, 0.95]),
    ("High noise", [1.0, 0.45, 1.0]),
];

/// What the LAS spec calls `class`.
pub fn name(class: u8) -> String {
    match NAMED.get(usize::from(class)) {
        Some((name, _)) => (*name).to_owned(),
        None if class < 64 => format!("Reserved {class}"),
        None => format!("User class {class}"),
    }
}

/// The colour to draw `class` in, as straight RGB.
pub fn color(class: u8) -> [f32; 3] {
    match NAMED.get(usize::from(class)) {
        Some((_, color)) => *color,
        // Unnamed classes get evenly spread hues, so neighbouring numbers
        // still look different.
        None => hue(f32::from(class) * 0.618_034),
    }
}

/// Every class's colour, indexed by class, padded to `vec4` for a uniform.
pub fn palette() -> [[f32; 4]; 256] {
    std::array::from_fn(|class| {
        let [r, g, b] = color(class as u8);
        [r, g, b, 1.0]
    })
}

/// A fully saturated, fairly light colour at `turns` around the hue circle.
fn hue(turns: f32) -> [f32; 3] {
    let channel = |offset: f32| {
        let k = (turns.fract() * 6.0 + offset) % 6.0;
        // HSV with value 0.9 and saturation 0.6.
        0.9 * (1.0 - 0.6 * k.min(4.0 - k).clamp(0.0, 1.0))
    };
    [channel(5.0), channel(3.0), channel(1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_should_say_whether_an_unnamed_class_is_reserved() {
        assert_eq!(name(2), "Ground");
        assert_eq!(name(40), "Reserved 40");
        assert_eq!(name(200), "User class 200");
    }

    #[test]
    fn hue_should_be_red_at_zero() {
        let [r, g, b] = hue(0.0);
        assert!((r - 0.9).abs() < 1e-6 && (g - 0.36).abs() < 1e-6 && (b - 0.36).abs() < 1e-6);
    }
}

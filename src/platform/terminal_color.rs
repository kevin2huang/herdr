pub(crate) fn ghostty_image_color(rgb: [u8; 3]) -> [u8; 3] {
    if !cfg!(target_os = "macos") {
        return rgb;
    }
    // Ghostty's Metal text shader converts sRGB to Display P3; its image shader
    // does not. Encode chrome images in that output space to match text colors.
    let [r, g, b] = rgb.map(|value| {
        let value = f64::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    });
    [
        0.82259287 * r + 0.17753395 * g,
        0.03319951 * r + 0.96678350 * g,
        0.01708535 * r + 0.07239572 * g + 0.91030148 * b,
    ]
    .map(|value| {
        let encoded = if value <= 0.0031308 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        };
        (encoded * 255.0).round().clamp(0.0, 255.0) as u8
    })
}

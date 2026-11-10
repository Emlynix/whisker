//! The two bundled typefaces, subset to Latin (SIL Open Font License,
//! texts in `assets/fonts`). Scripts they lack (Japanese, Korean,
//! Chinese) fall back to the system's fonts.

/// Family names as the files declare them.
pub const UI_FAMILY: &str = "Inter";
pub const MONO_FAMILY: &str = "JetBrains Mono";

/// File name (for backends that load fonts from disk) and content.
pub const FILES: [(&str, &[u8]); 5] = [
    ("Inter-Regular.ttf", include_bytes!("../assets/fonts/Inter-Regular.ttf")),
    ("Inter-Medium.ttf", include_bytes!("../assets/fonts/Inter-Medium.ttf")),
    ("Inter-SemiBold.ttf", include_bytes!("../assets/fonts/Inter-SemiBold.ttf")),
    ("JetBrainsMono-Medium.ttf", include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf")),
    ("JetBrainsMono-SemiBold.ttf", include_bytes!("../assets/fonts/JetBrainsMono-SemiBold.ttf")),
];

//! Account avatar colors.

/// The avatar hues, in the order accounts get them.
pub const AVATAR_HUES: [&str; 6] = [
    "#714CB6", "#D6478A", "#DB7A2B", "#2E9C6A", "#2A8CC4", "#B08A2E",
];

/// The hue for an account's stored hue slot. A new account takes the slot
/// after the highest one in use, so slots wrap round the palette.
#[must_use]
pub fn avatar_hue(slot: i64) -> &'static str {
    let len = i64::try_from(AVATAR_HUES.len()).unwrap_or(1);
    usize::try_from(slot.rem_euclid(len))
        .ok()
        .and_then(|i| AVATAR_HUES.get(i))
        .copied()
        .unwrap_or(AVATAR_HUES[0])
}

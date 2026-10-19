//! Embedded picker SVGs. Lucide and local chevron licenses are in assets/picker-icons.
use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

pub struct PickerAssets;
impl AssetSource for PickerAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "picker-icons/chevron-left.svg" => {
                include_bytes!("../../assets/picker-icons/chevron-left.svg")
            }
            "picker-icons/chevron-right.svg" => {
                include_bytes!("../../assets/picker-icons/chevron-right.svg")
            }
            "picker-icons/layout-grid.svg" => {
                include_bytes!("../../assets/picker-icons/layout-grid.svg")
            }
            "picker-icons/smile.svg" => include_bytes!("../../assets/picker-icons/smile.svg"),
            "picker-icons/clock.svg" => include_bytes!("../../assets/picker-icons/clock.svg"),
            "picker-icons/hand.svg" => include_bytes!("../../assets/picker-icons/hand.svg"),
            "picker-icons/leaf.svg" => include_bytes!("../../assets/picker-icons/leaf.svg"),
            "picker-icons/apple.svg" => include_bytes!("../../assets/picker-icons/apple.svg"),
            "picker-icons/plane.svg" => include_bytes!("../../assets/picker-icons/plane.svg"),
            "picker-icons/trophy.svg" => include_bytes!("../../assets/picker-icons/trophy.svg"),
            "picker-icons/lightbulb.svg" => {
                include_bytes!("../../assets/picker-icons/lightbulb.svg")
            }
            "picker-icons/hash.svg" => include_bytes!("../../assets/picker-icons/hash.svg"),
            "picker-icons/flag.svg" => include_bytes!("../../assets/picker-icons/flag.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok([
            "picker-icons/chevron-left.svg",
            "picker-icons/chevron-right.svg",
            "picker-icons/layout-grid.svg",
            "picker-icons/smile.svg",
            "picker-icons/clock.svg",
            "picker-icons/hand.svg",
            "picker-icons/leaf.svg",
            "picker-icons/apple.svg",
            "picker-icons/plane.svg",
            "picker-icons/trophy.svg",
            "picker-icons/lightbulb.svg",
            "picker-icons/hash.svg",
            "picker-icons/flag.svg",
        ]
        .into_iter()
        .filter(|name| name.starts_with(path))
        .map(Into::into)
        .collect())
    }
}

//! The TV picture's resolution people choose in the launcher and the
//! cast picker (docs/playshare-design.md §9.1). The TV page keeps its
//! layout size (`tv` in the manifest); tvpipe renders it at the chosen
//! resolution (a higher device scale factor), so the layout is the same
//! at 720p and at 4K. [`Resolution`] itself is shared with `share URL
//! --resolution` (`crate::types`).

pub use crate::types::Resolution;

/// What a cast asks of the TV picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picture {
    /// The TV page's layout size (`tv`); `None`: the receivers' shape.
    pub layout: Option<(u32, u32)>,
    /// The chosen resolution; `None`: exactly `layout` (`tv.resolution`
    /// `"fixed"`).
    pub resolution: Option<Resolution>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for r in Resolution::ALL {
            assert_eq!(Resolution::parse(r.as_str()), Some(r));
        }
        assert_eq!(Resolution::parse("4K"), Some(Resolution::P2160));
        assert_eq!(Resolution::parse("8k"), None);
        assert_eq!(Resolution::default(), Resolution::Auto);
        assert_eq!(Resolution::P2160.bounds(), (3840, 2160));
        assert_eq!(Resolution::Auto.bounds(), crate::types::AUTO_MAX);
        assert!(!Resolution::Auto.explicit() && Resolution::P720.explicit());
    }
}

use std::sync::OnceLock;

/// Global theme instance, initialized once at startup
static THEME: OnceLock<Theme> = OnceLock::new();

/// RGB color tuple
type Rgb = (u8, u8, u8);

/// Color theme for the TUI application
#[derive(Debug, Clone)]
pub struct Theme {
    // Primary accent (teal family)
    pub accent: Rgb,
    pub accent_dim: Rgb,

    // Text colors
    pub text_primary: Rgb,
    pub text_secondary: Rgb,
    pub text_muted: Rgb,

    // Structural
    pub border: Rgb,

    // Backgrounds
    pub status_bar_bg: Rgb,
    pub overlay_bg: Rgb,
    pub selection_bg: Rgb,

    // Semantic colors
    pub diff_add: Rgb,
    pub diff_remove: Rgb,
    pub code_color: Rgb,
    pub heading: Rgb,
    pub thinking_text: Rgb,
    pub tool_text: Rgb,

    // List view specific
    pub custom_title: Rgb,
    pub custom_title_highlight: Rgb,
    pub summary: Rgb,
    pub summary_highlight: Rgb,
    pub model_color: Rgb,
    pub duration_color: Rgb,
    pub preview: Rgb,
    pub context_base: Rgb,
    pub context_highlight: Rgb,

    // List metadata
    pub msg_count: Rgb,
    pub header_summary: Rgb,
    pub timestamp_now: Rgb,
    pub timestamp_minutes: Rgb,
    pub timestamp_hours: Rgb,
    pub timestamp_days: Rgb,

    // Disabled/dim states
    pub dim_key: Rgb,
    pub dim_label: Rgb,

    // Search
    pub search_match_bg: Rgb,

    // Viewer colors
    pub green: Rgb,
    pub blue: Rgb,

    /// Project name hues; a project keeps its hue across launches
    /// (see [`Theme::project_color`]).
    pub project_palette: [Rgb; 8],
    /// A worktree suffix (`repo/worktree`) and other quiet name parts.
    pub project_suffix: Rgb,

    // Syntect theme name for code highlighting
    pub syntect_theme: &'static str,
}

impl Theme {
    /// Dark theme - the original color scheme
    pub fn dark() -> Self {
        Self {
            accent: (78, 201, 176),
            accent_dim: (60, 160, 140),

            text_primary: (255, 255, 255),
            text_secondary: (140, 140, 140),
            text_muted: (100, 100, 100),

            border: (60, 60, 60),

            status_bar_bg: (30, 30, 35),
            overlay_bg: (25, 25, 30),
            selection_bg: (45, 45, 55),

            diff_add: (120, 200, 120),
            diff_remove: (220, 120, 120),
            code_color: (147, 161, 199),
            heading: (180, 190, 200),
            thinking_text: (140, 145, 150),
            tool_text: (140, 145, 150),

            custom_title: (200, 180, 120),
            custom_title_highlight: (230, 210, 150),
            summary: (195, 202, 214),
            summary_highlight: (235, 240, 250),
            model_color: (180, 140, 200),
            duration_color: (100, 140, 130),
            preview: (130, 130, 130),
            context_base: (100, 100, 100),
            context_highlight: (60, 160, 140),

            msg_count: (110, 110, 110),
            header_summary: (180, 180, 180),
            timestamp_now: (78, 201, 176), // Bright teal (same as accent)
            timestamp_minutes: (90, 175, 160), // Soft teal
            timestamp_hours: (130, 155, 150), // Muted teal-gray
            timestamp_days: (140, 140, 140), // Same as text_secondary

            dim_key: (60, 60, 60),
            dim_label: (60, 60, 60),

            search_match_bg: (78, 201, 176),

            green: (0, 255, 0),
            blue: (100, 149, 237),

            project_palette: [
                (97, 175, 239),  // blue
                (152, 195, 121), // green
                (229, 192, 123), // amber
                (198, 120, 221), // violet
                (86, 182, 194),  // cyan
                (224, 108, 117), // rose
                (209, 154, 102), // orange
                (170, 160, 240), // lavender
            ],
            project_suffix: (120, 125, 135),

            syntect_theme: "base16-ocean.dark",
        }
    }

    /// Light theme - designed for light terminal backgrounds
    pub fn light() -> Self {
        Self {
            accent: (13, 128, 118),     // Deep teal - legible on white
            accent_dim: (45, 115, 105), // Muted teal for secondary elements

            text_primary: (36, 45, 53),     // Deep slate for body text
            text_secondary: (88, 101, 112), // Cool gray for metadata
            text_muted: (130, 140, 148),    // Light gray for labels

            border: (188, 196, 200), // Subtle cool gray borders

            status_bar_bg: (238, 241, 244), // Very light cool gray
            overlay_bg: (246, 248, 249),    // Near-white for modals
            selection_bg: (221, 235, 232),  // Pale teal wash for selection

            diff_add: (40, 120, 60),        // Dark green for additions
            diff_remove: (180, 50, 50),     // Dark red for removals
            code_color: (80, 70, 130),      // Dark purple-blue
            heading: (52, 70, 100),         // Dark slate navy
            thinking_text: (110, 118, 128), // Cool medium gray
            tool_text: (96, 108, 118),      // Slightly cool gray

            custom_title: (140, 105, 30),           // Deep warm gold
            custom_title_highlight: (170, 130, 40), // Brighter gold
            summary: (50, 62, 78),                  // Near-body slate: the row's headline
            summary_highlight: (20, 40, 75),        // Deeper slate for highlights
            model_color: (115, 75, 145),            // Deep purple
            duration_color: (45, 115, 105),         // Teal-green (matches accent_dim)
            preview: (108, 116, 124),               // Cool medium gray
            context_base: (120, 130, 138),          // Light-medium gray
            context_highlight: (13, 128, 118),      // Same as accent

            msg_count: (105, 115, 122),        // Cool medium gray
            header_summary: (88, 101, 112),    // Matches text_secondary
            timestamp_now: (13, 128, 118),     // Same as accent
            timestamp_minutes: (30, 115, 105), // Soft teal
            timestamp_hours: (60, 100, 95),    // Muted teal-gray
            timestamp_days: (88, 101, 112),    // Same as text_secondary

            dim_key: (180, 188, 194), // Light for disabled
            dim_label: (180, 188, 194),

            search_match_bg: (194, 226, 220), // Pale teal wash for matches

            green: (40, 130, 60), // Dark green for quotes
            blue: (36, 97, 160),  // Dark blue for links

            project_palette: [
                (30, 100, 180), // blue
                (50, 120, 40),  // green
                (150, 100, 0),  // amber
                (140, 50, 160), // violet
                (0, 120, 140),  // cyan
                (180, 50, 60),  // rose
                (170, 85, 20),  // orange
                (90, 80, 180),  // lavender
            ],
            project_suffix: (120, 130, 138),

            syntect_theme: "InspiredGitHub",
        }
    }
}

impl Theme {
    /// The hue for a project name. Worktrees (`repo/branch`) share their
    /// repo's hue. FNV-1a, not `RandomState`, so it is stable across runs,
    /// then the splitmix64 finalizer: FNV's low 3 bits (the palette index)
    /// depend only on the low 3 bits of each byte, which throws most of a
    /// name away (`Work` and `h007` shared a hue without mixing).
    pub fn project_color(&self, project: &str) -> Rgb {
        let repo = project.split('/').next().unwrap_or(project);
        let mut hash = repo.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
        hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        hash ^= hash >> 31;
        self.project_palette[(hash % self.project_palette.len() as u64) as usize]
    }
}

/// `[display].theme`: pick a palette instead of asking the terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    Auto,
    Dark,
    Light,
}

/// Fix the theme for the process. Call once, before raw mode: `Auto` probes
/// the terminal. Later calls (and [`detect_theme`]) return the first choice.
pub fn init_theme(choice: ThemeChoice) -> &'static Theme {
    match choice {
        ThemeChoice::Auto => detect_theme(),
        ThemeChoice::Dark => THEME.get_or_init(Theme::dark),
        ThemeChoice::Light => THEME.get_or_init(Theme::light),
    }
}

fn stdout_is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Detect terminal background luminance and return appropriate theme
pub fn detect_theme() -> &'static Theme {
    THEME.get_or_init(|| {
        // terminal-light sends its query through stdout. Avoid probing when
        // stdout is redirected so the response cannot become part of a
        // command's captured output. Tests never probe, which keeps them
        // independent of the terminal that launched them.
        if cfg!(test) || !stdout_is_terminal() {
            return Theme::dark();
        }
        match terminal_light::luma() {
            Ok(luma) if luma > 0.6 => Theme::light(),
            _ => Theme::dark(), // Default to dark on detection failure
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: usize = 7;

    #[test]
    fn project_hue_is_stable_and_shared_by_worktrees() {
        let theme = Theme::dark();
        assert_eq!(
            theme.project_color("claude-history"),
            theme.project_color("claude-history/fix-resume")
        );
        // Pinned so a hasher change (which would recolour every project for
        // the user) is a deliberate test edit.
        let index = |name: &str| {
            let color = theme.project_color(name);
            theme
                .project_palette
                .iter()
                .position(|&c| c == color)
                .unwrap()
        };
        assert_eq!(index("claude-history"), PINNED);
        let mut counts = [0usize; 8];
        for n in 0..256 {
            counts[index(&format!("project-{n}"))] += 1;
        }
        // 32 per hue on average; a weak index would pile names onto a few.
        assert!(
            counts.iter().all(|&count| (16..=48).contains(&count)),
            "{counts:?}"
        );
        assert_ne!(index("Work"), index("h007"));
    }
}

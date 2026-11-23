//! A display name for a program the user picked: from the exe's version
//! info (Windows), else from its file name.

use std::path::Path;

use crate::emulators::display_name;

/// The longest name taken from an exe (in chars).
const MAX_NAME: usize = 128;

/// The version-info strings a name can come from. They are whatever the
/// exe's author wrote: untrusted, cleaned by [`choose_name`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionInfo {
    pub file_description: Option<String>,
    pub product_name: Option<String>,
}

/// Names that say which engine or runtime built the exe, not which game it
/// is (compared lowercase, after cleaning).
const NOT_A_GAME: &[&str] = &[
    "app",
    "application",
    "bootstrappackagedgame",
    "electron",
    "game",
    "godot engine",
    "java platform se binary",
    "launcher",
    "microsoft windows operating system",
    "nw.js",
    "openjdk platform binary",
    "ue4 game",
    "ue5 game",
    "unity player",
    "unityplayer",
    "unreal engine",
];

/// A name for the program at `path`: its version info if that names it,
/// else its tidied file name (empty if it has none).
pub fn exe_name(path: &Path) -> String {
    let info = crate::platform::version_info(path).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    choose_name(&info, &stem)
}

/// The exe's `FileDescription`, else its `ProductName`, else
/// [`display_name`] of the file stem.
///
/// FileDescription first: it describes this one exe, and games ship it as
/// the game's title ("Hades", "Celeste"). ProductName more often names
/// something bigger than the exe: an engine ("Unity Player"), a publisher's
/// suite, or Windows itself. Either is skipped when it is just an engine or
/// runtime name (see [`NOT_A_GAME`]).
pub fn choose_name(info: &VersionInfo, stem: &str) -> String {
    [&info.file_description, &info.product_name]
        .into_iter()
        .flatten()
        .map(|s| clean(s))
        .find(|s| {
            s.chars().any(char::is_alphanumeric) && !NOT_A_GAME.contains(&s.to_lowercase().as_str())
        })
        .unwrap_or_else(|| display_name(&stem.replace(is_format, "")))
}

/// `s` without control characters, trademark marks and extra spaces, at
/// most [`MAX_NAME`] chars.
fn clean(s: &str) -> String {
    // Bound the work on a huge string before anything else.
    let s: String = s.chars().take(4 * MAX_NAME).collect();
    let mut s = s.replace(['™', '®', '©', '℠'], "");
    for mark in ["(tm)", "(r)", "(c)"] {
        while let Some(i) = s.to_ascii_lowercase().find(mark) {
            s.replace_range(i..i + mark.len(), "");
        }
    }
    let s = s.replace(char::is_control, " ");
    // Invisible direction and format marks could disguise the name.
    let s = s.replace(is_format, "");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.chars()
        .take(MAX_NAME)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Bidi controls, zero-width characters and the BOM: invisible, and able
/// to make a name read differently from what it is.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{061C}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(description: Option<&str>, product: Option<&str>) -> VersionInfo {
        VersionInfo {
            file_description: description.map(String::from),
            product_name: product.map(String::from),
        }
    }

    #[test]
    fn the_description_comes_first_then_the_product_then_the_file_name() {
        let both = info(Some("Hades"), Some("Hades II Suite"));
        assert_eq!(choose_name(&both, "Hades"), "Hades");
        assert_eq!(choose_name(&info(None, Some("Celeste")), "x"), "Celeste");
        assert_eq!(
            choose_name(&info(Some(""), Some(" ")), "Celeste"),
            "Celeste"
        );
        assert_eq!(
            choose_name(&VersionInfo::default(), "Sky_Drifter [v1.2]"),
            "Sky_Drifter"
        );
        assert_eq!(choose_name(&VersionInfo::default(), ""), "");
    }

    #[test]
    fn engine_and_runtime_names_are_skipped() {
        let unity = info(Some("Unity Player"), Some("Hollow Knight"));
        assert_eq!(choose_name(&unity, "hollow_knight"), "Hollow Knight");
        let unreal = info(Some("BootstrapPackagedGame"), Some("BootstrapPackagedGame"));
        assert_eq!(choose_name(&unreal, "OuterWilds"), "OuterWilds");
        let java = info(Some("Java(TM) Platform SE binary"), None);
        assert_eq!(choose_name(&java, "javaw"), "javaw");
        let windows = info(None, Some("Microsoft® Windows® Operating System"));
        assert_eq!(choose_name(&windows, "notepad"), "notepad");
    }

    #[test]
    fn untrusted_strings_are_cleaned_and_capped() {
        let marked = info(Some("  Slay\tthe\u{0}Spire™ (TM) "), None);
        assert_eq!(choose_name(&marked, "x"), "Slay the Spire");
        assert_eq!(
            choose_name(&info(Some("Portal®  2"), None), "x"),
            "Portal 2"
        );
        let long = "A".repeat(10_000);
        assert_eq!(choose_name(&info(Some(&long), None), "x").len(), MAX_NAME);
        // Nothing but punctuation is not a name.
        assert_eq!(choose_name(&info(Some("-- ."), None), "doom"), "doom");
        // Multi-byte text survives the cap on a char boundary.
        let wide = "ö".repeat(500);
        assert_eq!(
            choose_name(&info(Some(&wide), None), "x").chars().count(),
            MAX_NAME
        );
    }

    #[test]
    fn bidi_and_zero_width_marks_are_removed() {
        let tricky = "\u{202E}Slay\u{200B} the\u{2066} Spire\u{2069}\u{200F}\u{FEFF}";
        assert_eq!(
            choose_name(&info(Some(tricky), None), "x"),
            "Slay the Spire"
        );
        // Marks alone are not a name.
        let marks = info(Some("\u{202A}\u{200E}"), Some("\u{2067}"));
        assert_eq!(choose_name(&marks, "do\u{202E}om"), "doom");
    }

    #[test]
    fn a_missing_exe_falls_back_to_its_file_name() {
        let path = Path::new("no-such-dir/Into the Breach (x64).exe");
        assert_eq!(exe_name(path), "Into the Breach");
    }
}

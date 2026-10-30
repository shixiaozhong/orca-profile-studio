//! Manifest `sub_path` values. They are written with `/` on every platform and compared as paths,
//! component by component, so neither depends on this platform's separator.

use std::path::{Path, PathBuf};

/// The manifest form of `path`, a path relative to the vendor directory.
pub(crate) fn to_manifest(path: &Path) -> String {
    path.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// The file a manifest `sub_path` names, relative to the vendor directory, in this platform's form.
pub(crate) fn to_path(sub_path: &str) -> PathBuf {
    Path::new(sub_path).components().collect()
}

/// Whether a manifest `sub_path` names `path` (relative to the vendor directory).
pub(crate) fn names(sub_path: &str, path: &Path) -> bool {
    Path::new(sub_path) == path
}

/// A case-folded key for finding paths that collide on case-insensitive file systems.
pub(crate) fn folded(path: &Path) -> String {
    to_manifest(path).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_slashes_and_compares_components() {
        let nested = Path::new("filament").join("Acme").join("PLA.json");
        assert_eq!(to_manifest(&nested), "filament/Acme/PLA.json");
        assert_eq!(to_path("filament/Acme/PLA.json"), nested);
        assert!(names("filament/Acme/PLA.json", &nested));
        assert!(names("filament//Acme/PLA.json", &nested));
        assert!(!names("filament/PLA.json", &nested));
        assert_eq!(folded(&Path::new("Vendor").join(&nested)), "vendor/filament/acme/pla.json");
    }

    #[cfg(windows)]
    #[test]
    fn reads_backslashes_on_windows() {
        let native = Path::new(r"filament\Acme\PLA.json");
        assert_eq!(to_manifest(native), "filament/Acme/PLA.json");
        assert_eq!(to_path("filament/Acme/PLA.json"), native);
        assert!(names("filament/Acme/PLA.json", native));
        assert_eq!(folded(native), folded(Path::new("filament/acme/pla.json")));
    }
}

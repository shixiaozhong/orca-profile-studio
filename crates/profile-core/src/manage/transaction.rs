use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use super::{ManageError, ManagementPlan, invalid};

pub(super) fn check_path(root: &Path, path: &Path) -> Result<(), ManageError> {
    if path.components().any(|part| !matches!(part, Component::Normal(_))) || path.as_os_str().is_empty() {
        return Err(invalid(format!("Path must stay inside the profiles directory: {}", path.display())));
    }
    let mut full = root.to_owned();
    for part in path.components() {
        full.push(part);
        match fs::symlink_metadata(&full) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(invalid(format!("Refusing a symbolic link: {}", full.display())));
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

pub(super) fn snapshot(root: &Path) -> Result<BTreeMap<PathBuf, String>, ManageError> {
    fn visit(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, String>) -> Result<(), ManageError> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if entry.file_name() == ".git"
                || entry.file_name().to_string_lossy().starts_with(".orca-profile-studio-transaction-")
            {
                continue;
            }
            if kind.is_symlink() {
                return Err(invalid(format!(
                    "Refusing to manage a directory containing a symbolic link: {}",
                    path.display()
                )));
            }
            if kind.is_dir() {
                visit(root, &path, files)?;
            } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")) {
                files.insert(
                    path.strip_prefix(root).map_err(|_| invalid("Path outside profiles"))?.to_owned(),
                    fs::read_to_string(path)?,
                );
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

/// Profiles directories being written by this process. An in-memory lock cannot go stale when
/// the app is killed; other processes are caught by the snapshot and per-file rechecks.
static WRITING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

struct Lock(PathBuf);
impl Lock {
    fn acquire(root: &Path) -> Result<Self, ManageError> {
        let mut writing = WRITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !writing.insert(root.to_owned()) {
            return Err(invalid("Another window is writing preset changes to this folder; try again when it finishes"));
        }
        Ok(Lock(root.to_owned()))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        WRITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&self.0);
    }
}

/// Stage first, then change preset files, then manifests. Retain recovery files if rollback fails.
pub(super) fn apply(plan: &ManagementPlan, fail_after: Option<usize>) -> Result<(), ManageError> {
    let _lock = Lock::acquire(&plan.root)?;
    if snapshot(&plan.root)? != plan.snapshot {
        return Err(invalid("Files changed since the preview; create a new preview"));
    }
    let stage = tempfile::Builder::new().prefix(".orca-profile-studio-transaction-").tempdir_in(&plan.root)?;
    let mut changes: Vec<_> = plan.changes.values().collect();
    // Removals go first, so a case-only rename frees the name before the new file is installed.
    changes.sort_by_key(|change| (change.path.components().count() == 1, change.after.is_some(), &change.path));
    let recovery: Vec<_> = changes
        .iter()
        .enumerate()
        .map(|(ix, change)| {
            serde_json::json!({
                "path":change.path,"backup":format!("{ix}.old"),"staged":format!("{ix}.new"),
                "had_original":change.before.is_some(),"will_exist":change.after.is_some()
            })
        })
        .collect();
    fs::write(
        stage.path().join("recovery.json"),
        serde_json::to_vec_pretty(&recovery).expect("recovery records serialize"),
    )?;
    for (ix, change) in changes.iter().enumerate() {
        check_path(&plan.root, &change.path)?;
        if let Some(after) = &change.after {
            let staged = stage.path().join(format!("{ix}.new"));
            fs::write(&staged, after)?;
            if change.before.is_some() {
                fs::set_permissions(staged, fs::metadata(plan.root.join(&change.path))?.permissions())?;
            }
        }
    }
    let mut backups = Vec::new();
    let mut installed = Vec::new();
    let mut created_dirs = Vec::new();
    let result = (|| {
        for (ix, change) in changes.iter().enumerate() {
            if fail_after == Some(ix) {
                return Err(invalid("Injected write failure"));
            }
            let target = plan.root.join(&change.path);
            check_path(&plan.root, &change.path)?;
            let current = match fs::read_to_string(&target) {
                Ok(text) => Some(text),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Err(err) => return Err(err.into()),
            };
            if current.as_ref() != change.before.as_ref() {
                return Err(invalid(format!("{} changed during writing", change.path.display())));
            }
            if change.before.is_some() {
                let backup = stage.path().join(format!("{ix}.old"));
                fs::rename(&target, &backup)?;
                backups.push((target.clone(), backup));
            }
            if change.after.is_some() {
                let mut missing = Vec::new();
                let mut parent = target.parent().unwrap();
                while !parent.exists() {
                    missing.push(parent.to_owned());
                    parent = parent.parent().unwrap();
                }
                for dir in missing.into_iter().rev() {
                    fs::create_dir(&dir)?;
                    created_dirs.push(dir);
                }
                install(&stage.path().join(format!("{ix}.new")), &target)?;
                installed.push(target);
            }
        }
        Ok(())
    })();
    if let Err(err) = result {
        let mut rollback = Vec::new();
        for path in installed.into_iter().rev() {
            if let Err(err) = fs::remove_file(&path) {
                rollback.push(err.to_string());
            }
        }
        for (target, backup) in backups.into_iter().rev() {
            if let Err(err) = install(&backup, &target) {
                rollback.push(err.to_string());
            }
        }
        for dir in created_dirs.into_iter().rev() {
            let _ = fs::remove_dir(dir);
        }
        if !rollback.is_empty() {
            let recovery = stage.keep();
            return Err(invalid(format!(
                "{err}. Rollback could not finish: {}. Originals are kept in {}",
                rollback.join("; "),
                recovery.display()
            )));
        }
        return Err(err);
    }
    Ok(())
}

/// Puts `source` at `target` without replacing an existing file. A hard link does it atomically on
/// the same file system; where links are unsupported (FAT/exFAT drives, some network shares) the
/// bytes are copied into a newly created file instead.
fn install(source: &Path, target: &Path) -> std::io::Result<()> {
    install_with(|source, target| fs::hard_link(source, target), source, target)
}

fn install_with(link: fn(&Path, &Path) -> std::io::Result<()>, source: &Path, target: &Path) -> std::io::Result<()> {
    match link(source, target) {
        Err(err) if err.kind() != std::io::ErrorKind::AlreadyExists => {
            let mut file = fs::OpenOptions::new().write(true).create_new(true).open(target)?;
            std::io::Write::write_all(&mut file, &fs::read(source)?).inspect_err(|_| {
                let _ = fs::remove_file(target);
            })
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::{Lock, install_with};
    use std::io::{Error, ErrorKind};
    use std::path::Path;

    #[test]
    fn installing_never_replaces_an_existing_file_with_or_without_hard_links() {
        let unsupported = |_: &Path, _: &Path| Err(Error::new(ErrorKind::Unsupported, "no hard links"));
        for linked in [true, false] {
            let install = |source: &Path, target: &Path| {
                if linked {
                    install_with(|source, target| std::fs::hard_link(source, target), source, target)
                } else {
                    install_with(unsupported, source, target)
                }
            };
            let dir = tempfile::tempdir().unwrap();
            let (source, target, other) =
                (dir.path().join("staged"), dir.path().join("target"), dir.path().join("other"));
            std::fs::write(&source, "new").unwrap();
            std::fs::write(&other, "other").unwrap();
            install(&source, &target).unwrap();
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
            assert_eq!(install(&other, &target).unwrap_err().kind(), ErrorKind::AlreadyExists);
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        }
    }

    #[test]
    fn the_write_lock_is_per_folder_and_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let held = Lock::acquire(dir.path()).unwrap();
        assert!(Lock::acquire(dir.path()).is_err(), "a second writer to the same folder waits");
        drop(Lock::acquire(other.path()).unwrap());
        drop(held);
        drop(Lock::acquire(dir.path()).unwrap());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

//! File-system operations behind the sidebar's context menu. Blocking; the app
//! runs them on a worker. Names typed by the user are validated here so that
//! nothing can escape the directory it was meant for.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

/// What happened, for the app to update tabs / notices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    Created { path: PathBuf, is_dir: bool },
    Renamed { from: PathBuf, to: PathBuf },
    Trashed(PathBuf),
}

/// A relative path made of normal components only (`src/a.php` is fine,
/// `../x`, `/etc/x` and empty names are not).
fn relative(name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a name".to_owned());
    }
    let path = PathBuf::from(name);
    if path.components().any(|c| !matches!(c, Component::Normal(_))) || name.contains('\0') {
        return Err(format!("\"{name}\" is not a valid name"));
    }
    Ok(path)
}

/// Creates an empty file (and missing parent folders) below `dir`.
pub fn create_file(dir: &Path, name: &str) -> Result<Done, String> {
    let path = dir.join(relative(name)?);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => format!("\"{name}\" already exists"),
            _ => e.to_string(),
        })?;
    Ok(Done::Created { path, is_dir: false })
}

pub fn create_dir(dir: &Path, name: &str) -> Result<Done, String> {
    let path = dir.join(relative(name)?);
    if path.exists() {
        return Err(format!("\"{name}\" already exists"));
    }
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(Done::Created { path, is_dir: true })
}

/// Renames within the same directory (no moving via `/`).
pub fn rename(path: &Path, new_name: &str) -> Result<Done, String> {
    let new_name = new_name.trim();
    let single = relative(new_name)?;
    if single.components().count() != 1 {
        return Err("A name can't contain \"/\"".to_owned());
    }
    let to = path.with_file_name(new_name);
    if to == path {
        return Ok(Done::Renamed { from: path.to_path_buf(), to });
    }
    if to.exists() {
        return Err(format!("\"{new_name}\" already exists"));
    }
    fs::rename(path, &to).map_err(|e| e.to_string())?;
    Ok(Done::Renamed { from: path.to_path_buf(), to })
}

/// Moves to the desktop trash (recoverable); never deletes permanently.
pub fn trash(path: &Path) -> Result<Done, String> {
    trash::delete(path).map_err(|e| format!("Could not move to trash: {e}"))?;
    Ok(Done::Trashed(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("veloedit-fileops-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn creates_files_and_folders_with_nested_names() {
        let d = dir("create");
        assert_eq!(
            create_file(&d, " a/b/new.php ").unwrap(),
            Done::Created { path: d.join("a/b/new.php"), is_dir: false }
        );
        assert!(d.join("a/b/new.php").is_file());
        assert!(create_file(&d, "a/b/new.php").unwrap_err().contains("already exists"));
        assert!(create_dir(&d, "a/b").unwrap_err().contains("already exists"));
        create_dir(&d, "x/y").unwrap();
        assert!(d.join("x/y").is_dir());
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn rejects_names_that_could_escape_the_folder() {
        let d = dir("escape");
        for bad in ["", "   ", "../evil", "/etc/evil", "a/../../evil", "./x/../.."] {
            assert!(create_file(&d, bad).is_err(), "{bad:?}");
            assert!(create_dir(&d, bad).is_err(), "{bad:?}");
        }
        let f = d.join("f.txt");
        fs::write(&f, "x").unwrap();
        for bad in ["a/b", "..", "/x", ""] {
            assert!(rename(&f, bad).is_err(), "{bad:?}");
        }
        assert!(f.exists());
        fs::remove_dir_all(d).ok();
    }

    #[test]
    fn renames_in_place_and_refuses_to_overwrite() {
        let d = dir("rename");
        fs::write(d.join("a.txt"), "A").unwrap();
        fs::write(d.join("b.txt"), "B").unwrap();
        assert!(rename(&d.join("a.txt"), "b.txt").unwrap_err().contains("already exists"));
        assert_eq!(fs::read_to_string(d.join("b.txt")).unwrap(), "B", "untouched");
        assert_eq!(rename(&d.join("a.txt"), "c.txt").unwrap(), Done::Renamed { from: d.join("a.txt"), to: d.join("c.txt") });
        assert_eq!(fs::read_to_string(d.join("c.txt")).unwrap(), "A");
        // Same name: a no-op, not an error.
        assert!(rename(&d.join("c.txt"), "c.txt").is_ok());
        fs::remove_dir_all(d).ok();
    }
}

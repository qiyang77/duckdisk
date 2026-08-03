use std::fs;
use std::path::{Path, PathBuf};

pub fn delete_item(scan_root: &str, item_path: &str) -> Result<(), String> {
    let root = fs::canonicalize(scan_root)
        .map_err(|err| format!("Could not resolve scan root: {err}"))?;
    let item = resolve_without_following_final_symlink(item_path)?;

    if item == root || !item.starts_with(&root) {
        return Err("Refusing to delete outside the scanned path or delete its root".to_string());
    }

    let metadata = fs::symlink_metadata(&item)
        .map_err(|err| format!("Could not inspect item: {err}"))?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(&item).map_err(|err| format!("Could not delete item: {err}"))
    } else if metadata.is_dir() {
        fs::remove_dir_all(&item).map_err(|err| format!("Could not delete folder: {err}"))
    } else {
        Err("Refusing to delete a device or other special file".to_string())
    }
}

fn resolve_without_following_final_symlink(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    let parent = path
        .parent()
        .ok_or_else(|| "Item path has no parent directory".to_string())?;
    let name = path
        .file_name()
        .ok_or_else(|| "Item path has no file name".to_string())?;
    let parent = fs::canonicalize(parent)
        .map_err(|err| format!("Could not resolve item parent: {err}"))?;
    Ok(parent.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_test_directory(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("duckdisk-delete-test-{name}-{stamp}"))
    }

    #[test]
    fn deletes_items_only_inside_the_scan_root() {
        let base = temporary_test_directory("boundary");
        let root = base.join("root");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let inside_file = root.join("inside.txt");
        let outside_file = outside.join("outside.txt");
        fs::write(&inside_file, "inside").unwrap();
        fs::write(&outside_file, "outside").unwrap();

        delete_item(root.to_str().unwrap(), inside_file.to_str().unwrap()).unwrap();
        assert!(!inside_file.exists());
        assert!(delete_item(root.to_str().unwrap(), outside_file.to_str().unwrap()).is_err());
        assert!(outside_file.exists());
        assert!(delete_item(root.to_str().unwrap(), root.to_str().unwrap()).is_err());

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn rejects_paths_reached_through_a_parent_symlink() {
        let base = temporary_test_directory("symlink");
        let root = base.join("root");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let outside_file = outside.join("outside.txt");
        fs::write(&outside_file, "outside").unwrap();
        let link = root.join("link");
        symlink(&outside, &link).unwrap();

        let escaped = link.join("outside.txt");
        assert!(delete_item(root.to_str().unwrap(), escaped.to_str().unwrap()).is_err());
        assert!(outside_file.exists());

        fs::remove_dir_all(base).unwrap();
    }
}

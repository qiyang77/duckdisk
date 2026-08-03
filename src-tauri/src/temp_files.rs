use std::fs;
use std::path::PathBuf;

pub fn validate_result_file(path: &str, prefix: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    let temp_dir = std::env::temp_dir();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Temporary result path has no valid file name".to_string())?;

    if path.parent() != Some(temp_dir.as_path())
        || !file_name.starts_with(prefix)
        || !file_name.ends_with(".json")
    {
        return Err("Refusing to read an unrecognized temporary result file".to_string());
    }

    let metadata = fs::symlink_metadata(&path).map_err(|err| err.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Temporary result must be a regular file".to_string());
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn accepts_only_direct_regular_result_files() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("duckdisk-test-{stamp}.json"));
        fs::write(&path, "{}").unwrap();

        assert_eq!(
            validate_result_file(path.to_str().unwrap(), "duckdisk-test-").unwrap(),
            path
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_parent_traversal_and_wrong_prefixes() {
        let traversal = std::env::temp_dir().join("../duckdisk-test-result.json");
        assert!(validate_result_file(traversal.to_str().unwrap(), "duckdisk-test-").is_err());

        let unrelated = std::env::temp_dir().join("unrelated.json");
        assert!(validate_result_file(unrelated.to_str().unwrap(), "duckdisk-test-").is_err());
    }
}

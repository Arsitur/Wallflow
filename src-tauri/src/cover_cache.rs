//! Persistent generations keep old/in-flight thumbnail results out of a rebuilt cache.
use std::{fs, path::{Path,PathBuf}, time::{SystemTime,UNIX_EPOCH}};

pub fn generation(root: &Path) -> String {
    if !fs::metadata(root.join("generation")).is_ok_and(|metadata| metadata.len()==32) { return "scene-v5".into(); }
    fs::read(root.join("generation")).ok().filter(|bytes| bytes.len()==32)
        .and_then(|bytes| String::from_utf8(bytes).ok()).filter(|value| value.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_else(|| "scene-v5".into())
}
pub fn directory(root: &Path) -> PathBuf { root.join(format!("generation-{}",generation(root))) }

pub fn rebuild(root: &Path) -> Result<(),String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let id=format!("{:032x}",SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos());
    let temporary=root.join("generation.tmp");
    fs::write(&temporary,&id).map_err(|e| e.to_string())?;
    fs::rename(temporary,root.join("generation")).map_err(|e| e.to_string())?;
    // Only generated JPEGs and our generation directories live in this cache.
    // Workers already running may finish in an old directory, never the new one.
    for entry in fs::read_dir(root).map_err(|e| e.to_string())?.flatten() {
        let name=entry.file_name(); let name=name.to_string_lossy();
        let Ok(kind)=entry.file_type() else { continue; };
        if name.starts_with("generation-") && name!=format!("generation-{id}") && kind.is_dir() {
            let _=fs::remove_dir_all(entry.path());
        } else if name.ends_with(".jpg") && kind.is_file() { let _=fs::remove_file(entry.path()); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rebuild_persists_a_new_namespace_and_late_old_results_are_ignored() {
        let root=std::env::temp_dir().join(format!("wallflow-cache-test-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let old=directory(&root);
        fs::create_dir_all(&old).unwrap();fs::write(old.join("small.jpg"),b"old").unwrap();
        fs::write(root.join("preview.jpg"),b"legacy").unwrap();
        fs::write(root.join("unrelated.txt"),b"keep").unwrap();
        rebuild(&root).unwrap();
        let next=directory(&root);assert_ne!(next,old);assert_eq!(next,directory(&root));
        fs::create_dir_all(&old).unwrap();fs::write(old.join("late.jpg"),b"late").unwrap();
        assert!(!next.join("late.jpg").exists());assert!(!root.join("preview.jpg").exists());assert!(root.join("unrelated.txt").exists());
        rebuild(&root).unwrap();assert_ne!(directory(&root),next);
        let expected=std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(root.canonicalize().unwrap().parent(),Some(expected.as_path()));
        fs::remove_dir_all(root).unwrap();
    }
}

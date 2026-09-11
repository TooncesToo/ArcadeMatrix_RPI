//! Coverage for the GIF library helpers behind `/api/gifs/*` (pure logic, no server).
use arcadematrix::api::gifs::{
    gifs_bad, gifs_has_media_ext, gifs_list_folder, gifs_sanitize_name, gifs_scan_library_at,
    walkdir_count,
};
use std::fs;
use std::path::PathBuf;

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arcadematrix_gifs_test_{}_{}",
        tag,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn sanitize_keeps_plain_names_and_strips_paths() {
    assert_eq!(gifs_sanitize_name("Logo", false), "Logo");
    assert_eq!(gifs_sanitize_name("My Playlist-2", false), "My Playlist-2");
    assert_eq!(gifs_sanitize_name("../../etc/passwd", false), "passwd");
    assert_eq!(gifs_sanitize_name("C:\\evil\\name.gif", true), "name.gif");
    assert_eq!(gifs_sanitize_name(".hidden.gif", true), "hidden.gif");
    assert_eq!(gifs_sanitize_name("a/b.gif", false), "bgif"); // dots dropped when ext not allowed
    assert_eq!(
        gifs_sanitize_name("weird*chars?.gif", true),
        "weirdchars.gif"
    );
    assert_eq!(gifs_sanitize_name(&"x".repeat(100), false).len(), 64);
}

#[test]
fn media_extension_check() {
    assert!(gifs_has_media_ext("a.gif"));
    assert!(gifs_has_media_ext("A.GIF"));
    assert!(gifs_has_media_ext("b.png"));
    assert!(gifs_has_media_ext("c.raw"));
    assert!(!gifs_has_media_ext("notes.txt"));
    assert!(!gifs_has_media_ext("index"));
}

#[test]
fn raw_names_with_separators_are_rejected() {
    assert!(gifs_bad("../x"));
    assert!(gifs_bad("a/b"));
    assert!(gifs_bad("a\\b"));
    assert!(gifs_bad("..hidden"));
    assert!(!gifs_bad("Logo"));
    assert!(!gifs_bad("My Playlist"));
}

#[test]
fn list_folder_returns_only_media_sorted_and_skips_index_and_dotfiles() {
    let root = temp_root("list");
    let dir = root.join("Arcade");
    fs::create_dir_all(dir.join("sub")).unwrap();
    for name in [
        "b.gif",
        "a.png",
        "index.txt",
        ".DS_Store",
        "._a.gif",
        "readme.md",
        "c.RAW",
    ] {
        fs::write(dir.join(name), b"x").unwrap();
    }
    fs::write(dir.join("sub").join("nested.gif"), b"x").unwrap();
    let files = gifs_list_folder(&dir);
    assert_eq!(files, vec!["a.png", "b.gif", "c.RAW"]);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn scan_library_reports_folders_and_counts() {
    let root = temp_root("scan");
    fs::create_dir_all(root.join("Logo")).unwrap();
    fs::create_dir_all(root.join("Empty")).unwrap();
    fs::create_dir_all(root.join(".Trashes")).unwrap();
    fs::write(root.join("Logo").join("one.gif"), b"x").unwrap();
    fs::write(root.join("Logo").join("two.gif"), b"x").unwrap();
    fs::write(root.join("stray.gif"), b"x").unwrap(); // loose file at the root is not a folder
    let lib = gifs_scan_library_at(&root);
    assert_eq!(lib["root"], "/gifs");
    let folders = lib["folders"].as_object().unwrap();
    assert_eq!(folders.len(), 2, "hidden folders are skipped: {folders:?}");
    assert_eq!(folders["Logo"]["count"], 2);
    assert_eq!(folders["Logo"]["path"], "/gifs/Logo");
    assert_eq!(folders["Empty"]["count"], 0);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn walkdir_count_is_recursive() {
    let root = temp_root("walk");
    fs::create_dir_all(root.join("a").join("b")).unwrap();
    fs::write(root.join("x.gif"), b"x").unwrap();
    fs::write(root.join("a").join("y.gif"), b"x").unwrap();
    fs::write(root.join("a").join("b").join("z.gif"), b"x").unwrap();
    assert_eq!(walkdir_count(&root), 3);
    let _ = fs::remove_dir_all(&root);
}

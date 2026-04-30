use std::path::{Path, PathBuf};

#[test]
fn no_mcp_path_to_discovery() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_rs(&root.join("src/frontend"), &mut files);
    collect_rs(&root.join("src/session"), &mut files);

    for file in files {
        let input = std::fs::read_to_string(&file).expect("read source file");
        assert!(
            !input.contains("cli::init::discovery")
                && !input.contains("cli::init::ssh_exec")
                && !input.contains("init::discovery")
                && !input.contains("init::ssh_exec"),
            "{} must not import init discovery code",
            file.display()
        );
    }
}

fn collect_rs(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}
